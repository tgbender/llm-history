//! One retrieval pipeline shared by all front ends; engine details stay behind traits.
use anyhow::{Result, bail};
use llm_history_core::bounded;
use llm_history_models::{Embedder, Reranker};
use llm_history_store::Store;
use llm_history_types::query::Message;
use llm_history_types::query::{Filter, Mode, SearchRequest, Searched, ToolSearchResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    #[serde(flatten)]
    pub message: Message,
    pub score: f64,
    pub lexical_score: f64,
    pub semantic_score: f64,
    pub rerank_score: Option<f64>,
    /// A guardian review's outcome. Its prompt stays the hit, since that says what
    /// was reviewed and the verdict alone does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<Hit>,
    /// Every match before the result caps, known only for exact search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<usize>,
    /// Set when nothing matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searched: Option<Searched>,
    pub details: Value,
}

fn terms(s: &str) -> Vec<String> {
    let stopwords = "about after also and are did does for from how into its one our should that the their then these they this through use was were what when where which why with would";
    let mut seen = HashSet::new();
    s.split(|c: char| !c.is_alphanumeric() && !"_+#./:-".contains(c))
        .map(|s| s.trim_matches(['.', '/', ':', '-']).to_lowercase())
        .filter(|s| {
            s.chars().count() >= 3
                && !stopwords.split_whitespace().any(|w| w == s)
                && seen.insert(s.clone())
        })
        .collect()
}
pub fn eligible(m: &Message) -> bool {
    !m.excluded
        && m.message_kind == "conversation"
        && (m.role == "user" || (m.role == "assistant" && m.phase == "final_answer"))
}

/// Literal hits are ordered by date, so a model has no relevance to rerank. Front ends
/// call this before starting one.
pub fn check_rerank(mode: Mode, rerank: bool) -> Result<()> {
    if mode.is_literal() && rerank {
        bail!(
            "--rerank needs relevance-ranked hits; add --mode lexical, semantic or hybrid \
             (words and exact order hits by date)"
        );
    }
    Ok(())
}

pub fn search<S: Borrow<Store>>(
    open: impl Fn() -> Result<S>,
    request: &SearchRequest,
    mut embedder: Option<&mut dyn Embedder>,
    reranker: Option<&mut dyn Reranker>,
) -> Result<SearchResponse> {
    let started = Instant::now();
    if request.query.trim().is_empty() || request.limit == 0 || request.per_session == 0 {
        bail!("query, positive limit and per-session count are required");
    }
    check_rerank(request.mode, reranker.is_some())?;
    if request.mode == Mode::Semantic && request.filter.kind.as_deref() == Some("guardian") {
        bail!("guardian reviews are not embedded; search them lexically");
    }
    let mut embedding_error = None;
    let mut query_embedding = None;
    if matches!(request.mode, Mode::Auto | Mode::Semantic | Mode::Hybrid) {
        if let Some(embedder) = embedder.as_mut() {
            if request.filter.project.is_some() {
                open()?.borrow().validate_project_filter(&request.filter)?;
            }
            match embedder.embed(std::slice::from_ref(&request.query), true) {
                Ok(mut vectors) => {
                    if vectors.len() != 1 {
                        bail!("embedder returned wrong number of query vectors");
                    }
                    query_embedding = Some((embedder.profile().id.clone(), vectors.remove(0)));
                }
                Err(e) if request.mode == Mode::Auto => {
                    embedding_error = Some(llm_history_core::redact(&e.to_string()))
                }
                Err(e) => return Err(e),
            }
        } else if request.mode != Mode::Auto {
            bail!("semantic/hybrid search requires a configured embedding model");
        }
    }
    let mode = if request.mode == Mode::Auto {
        if query_embedding.is_some() {
            Mode::Hybrid
        } else {
            Mode::Lexical
        }
    } else {
        request.mode
    };
    let candidate_limit = (request.limit * 10).max(if reranker.is_some() {
        request.rerank_depth
    } else {
        0
    });
    let pool_limit = if mode == Mode::Hybrid {
        100.max(candidate_limit * 10)
    } else {
        candidate_limit
    };
    let connection = open()?;
    let store = connection.borrow();
    store.validate_project_filter(&request.filter)?;
    let counts = request
        .explain
        .then(|| {
            store.retrieval_counts(
                &request.filter,
                query_embedding.as_ref().map(|(id, _)| id.as_str()),
            )
        })
        .transpose()?;
    let mut matched = None;
    let (lexical, engine) = if mode == Mode::Semantic {
        (HashMap::new(), "unused".into())
    } else if mode.is_literal() {
        let (needles, engine) = if mode == Mode::Exact {
            (vec![request.query.clone()], "exact")
        } else {
            (words(&request.query), "words")
        };
        let (found, total) =
            store.literal_candidates(&needles, &request.filter, request.per_session, pool_limit)?;
        matched = Some(total);
        (found, engine.into())
    } else {
        store.lexical_candidates(&request.query, &request.filter, pool_limit)?
    };
    let semantic = if let Some((id, vector)) = &query_embedding {
        store.semantic_candidates(id, vector, &request.filter, pool_limit)?
    } else {
        HashMap::new()
    };
    let ids = lexical
        .keys()
        .chain(semantic.keys())
        .map(|id| id.parse::<i64>())
        .collect::<std::result::Result<HashSet<_>, _>>()?
        .into_iter()
        .collect::<Vec<_>>();
    let messages = store.messages_by_ids(&ids, &request.filter)?;
    let recency = |ts: &str| {
        chrono::DateTime::parse_from_rfc3339(ts)
            .map(|t| t.timestamp_millis())
            .unwrap_or(i64::MIN)
    };
    let timestamps: HashMap<_, _> = messages
        .iter()
        .map(|m| {
            (
                m.id.as_str(),
                recency(m.timestamp_utc.as_deref().unwrap_or(&m.ts)),
            )
        })
        .collect();
    let ordered = |map: &HashMap<String, f64>| {
        let mut v: Vec<_> = map.iter().collect();
        v.sort_by(|a, b| {
            b.1.total_cmp(a.1)
                .then(timestamps[b.0.as_str()].cmp(&timestamps[a.0.as_str()]))
                .then(a.0.cmp(b.0))
        });
        v.into_iter()
            .enumerate()
            .map(|(rank, (id, _))| (id.clone(), rank + 1))
            .collect::<HashMap<_, _>>()
    };
    let lexical_rank = ordered(&lexical);
    let semantic_rank = ordered(&semantic);
    let mut hits: Vec<_> = messages
        .iter()
        .cloned()
        .filter_map(|m| {
            let l = lexical.get(&m.id).copied().unwrap_or(0.);
            let s = semantic.get(&m.id).copied().unwrap_or(0.);
            let score = match mode {
                Mode::Auto | Mode::Words | Mode::Lexical | Mode::Exact => l,
                Mode::Semantic => *semantic.get(&m.id)?,
                Mode::Hybrid => {
                    lexical_rank
                        .get(&m.id)
                        .map(|r| 1. / (60. + *r as f64))
                        .unwrap_or(0.)
                        + semantic_rank
                            .get(&m.id)
                            .map(|r| 1. / (60. + *r as f64))
                            .unwrap_or(0.)
                }
            };
            if (mode != Mode::Semantic && score <= 0.) || !score.is_finite() {
                return None;
            }
            Some(Hit {
                message: m,
                score,
                lexical_score: l,
                semantic_score: s,
                rerank_score: None,
                verdict: None,
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(
                recency(b.message.timestamp_utc.as_deref().unwrap_or(&b.message.ts)).cmp(&recency(
                    a.message.timestamp_utc.as_deref().unwrap_or(&a.message.ts),
                )),
            )
            .then(a.message.id.cmp(&b.message.id))
    });
    hits.truncate(candidate_limit);
    // A ranked search over both roles shows the answer to a matching question. A literal
    // hit has to show the matched text, which the answer may lack.
    let answer_questions = request.filter.role.is_none() && !mode.is_literal();
    if request.filter.role.as_deref() != Some("assistant") {
        let users = hits
            .iter()
            .filter(|h| {
                h.message.role == "user"
                    && (answer_questions || h.message.session_kind == "guardian")
            })
            .map(|h| h.message.id.parse::<i64>())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // The answers are assistant messages, whatever role the search was limited to.
        let either_role = Filter {
            role: None,
            ..request.filter.clone()
        };
        let answers = store.answer_messages(&users, &either_role)?;
        let mut seen = HashSet::new();
        hits = hits
            .into_iter()
            .filter_map(|mut hit| {
                if let Some(answer) = answers.get(&hit.message.id) {
                    if hit.message.session_kind == "guardian" {
                        hit.verdict = Some(answer.text.clone());
                    } else if answer_questions {
                        hit.message = answer.clone();
                    }
                }
                if seen.insert(hit.message.id.clone()) {
                    Some(hit)
                } else {
                    None
                }
            })
            .collect();
    }
    let searched = hits
        .is_empty()
        .then(|| -> Result<Searched> {
            Ok(Searched {
                count: store.retrieval_counts(&request.filter, None)?.0,
                through: store.newest_message_at(&request.filter)?,
            })
        })
        .transpose()?;
    drop(connection);
    let reranked = reranker.is_some();
    let mut rerank_failures = 0;
    if let Some(reranker) = reranker {
        let depth = request.rerank_depth.min(hits.len());
        let documents: Vec<_> = hits[..depth]
            .iter()
            .map(|h| {
                passage(
                    searched_text(&h.message),
                    &request.query,
                    3200,
                    false,
                    false,
                )
            })
            .collect();
        let rerank_query = format!("{}{}", reranker.profile().query_prefix, request.query);
        let scores = if documents.is_empty() {
            llm_history_models::RerankResult {
                scores: Vec::new(),
                failed: Vec::new(),
            }
        } else {
            llm_history_models::rerank_resilient(reranker, &rerank_query, &documents)?
        };
        rerank_failures = scores.failed.len();
        let failed = scores.failed;
        let mut reordered = Vec::new();
        let mut seen = HashSet::new();
        for (index, score) in scores.scores {
            if index >= depth || !seen.insert(index) || !score.is_finite() {
                bail!("reranker contract violation");
            }
            let mut hit = hits[index].clone();
            hit.rerank_score = Some(score);
            hit.score = score;
            reordered.push(hit);
        }
        for index in failed {
            if index >= depth || !seen.insert(index) {
                bail!("reranker contract violation");
            }
            reordered.push(hits[index].clone());
        }
        if seen.len() != depth {
            bail!("reranker omitted candidates");
        }
        reordered.extend_from_slice(&hits[depth..]);
        hits = reordered;
    }
    let focus = match mode {
        Mode::Exact => vec![phrase(&request.query)],
        Mode::Words => words(&request.query),
        _ => terms(&request.query),
    };
    let mut sessions = HashMap::new();
    let mut result = Vec::new();
    let mut remaining = request.max_output_chars;
    for mut hit in hits {
        let count = sessions.entry(hit.message.session_id.clone()).or_insert(0);
        if *count >= request.per_session {
            continue;
        }
        *count += 1;
        let budget = |remaining: usize| {
            if request.full {
                usize::MAX
            } else {
                remaining.min(request.snippet_chars)
            }
        };
        if budget(remaining) == 0 {
            break;
        }
        if let Some(verdict) = &mut hit.verdict {
            *verdict = bounded(verdict, budget(remaining), request.redact);
            if !request.full {
                remaining = remaining.saturating_sub(verdict.chars().count());
            }
        }
        let text = if request.full {
            &hit.message.text
        } else {
            searched_text(&hit.message)
        };
        hit.message.text = passage_around(
            text,
            &focus,
            budget(remaining),
            request.redact,
            request.full,
        );
        if !request.full {
            remaining = remaining.saturating_sub(hit.message.text.chars().count());
        }
        result.push(hit);
        if result.len() == request.limit {
            break;
        }
    }
    let mut details = json!({"mode":request.mode,"retrieval":mode,"embedding_error":embedding_error,"lexical_engine":engine,"reranked":reranked,"rerank_failures":rerank_failures,"elapsed_seconds":started.elapsed().as_secs_f64(),"output_truncated":!request.full && remaining==0});
    if let Some((eligible, coverage)) = counts {
        details["eligible_documents"] = json!(eligible);
        details["embedded_documents"] = json!(coverage);
    }
    Ok(SearchResponse {
        results: result,
        matched,
        searched,
        details,
    })
}
/// Search the arguments of tool calls, such as shell commands, for literal text. Hits are
/// newest first and name the message before the call, which `show` opens.
pub fn search_tools<S: Borrow<Store>>(
    open: impl Fn() -> Result<S>,
    request: &SearchRequest,
) -> Result<ToolSearchResponse> {
    if request.query.trim().is_empty() || request.limit == 0 || request.per_session == 0 {
        bail!("query, positive limit and per-session count are required");
    }
    if !request.mode.is_literal() {
        bail!("tool calls are searched as literal text; use --mode words or --mode exact");
    }
    let (needles, focus) = if request.mode == Mode::Exact {
        (vec![request.query.clone()], vec![phrase(&request.query)])
    } else {
        (words(&request.query), words(&request.query))
    };
    let connection = open()?;
    let store = connection.borrow();
    store.validate_project_filter(&request.filter)?;
    let (calls, matched) = store.tool_call_candidates(
        &needles,
        &request.filter,
        request.per_session,
        request.limit,
    )?;
    let searched = calls
        .is_empty()
        .then(|| -> Result<Searched> {
            Ok(Searched {
                count: store.tool_call_candidates(&[], &request.filter, 1, 1)?.1,
                through: store.newest_message_at(&request.filter)?,
            })
        })
        .transpose()?;
    drop(connection);
    let mut remaining = request.max_output_chars;
    let mut results = Vec::new();
    for mut call in calls {
        let budget = if request.full {
            usize::MAX
        } else {
            remaining.min(request.snippet_chars)
        };
        if budget == 0 {
            break;
        }
        call.text = passage_around(&call.text, &focus, budget, request.redact, request.full);
        if !request.full {
            remaining = remaining.saturating_sub(call.text.chars().count());
        }
        results.push(call);
    }
    Ok(ToolSearchResponse {
        results,
        matched,
        searched,
    })
}
/// What the lexical index holds for a message, so snippets show why it matched.
fn searched_text(message: &Message) -> &str {
    (message.session_kind == "guardian")
        .then(|| llm_history_core::guardian_action(&message.text))
        .flatten()
        .unwrap_or(&message.text)
}
/// The distinct words of a query, lowercase: each one has to appear in a `words` hit.
fn words(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|word| seen.insert(word.clone()))
        .collect()
}
/// An exact query in the form passages search: lowercase, with single spaces.
fn phrase(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
pub fn passage(text: &str, query: &str, budget: usize, redact: bool, full: bool) -> String {
    passage_around(text, &terms(query), budget, redact, full)
}
fn passage_around(text: &str, terms: &[String], budget: usize, redact: bool, full: bool) -> String {
    let clean = if redact {
        llm_history_core::redact(text)
    } else {
        text.into()
    };
    if full || budget == 0 {
        return bounded(&clean, budget, false);
    }
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = clean.chars().collect::<Vec<_>>();
    if chars.len() <= budget {
        return clean;
    }
    // Map lowercase byte positions back to source characters, including Unicode
    // lowercase expansions, before choosing a bounded source passage.
    let mut lower = String::new();
    let mut offsets = Vec::new();
    for (index, ch) in chars.iter().enumerate() {
        let folded = ch.to_lowercase().collect::<String>();
        offsets.extend(std::iter::repeat_n(index, folded.len()));
        lower.push_str(&folded);
    }
    let positions = terms
        .iter()
        .flat_map(|term| lower.match_indices(term).map(|(at, _)| offsets[at]))
        .collect::<Vec<_>>();
    let best_start = |window: usize| {
        let candidates = positions
            .iter()
            .map(|at| {
                at.saturating_sub(window / 3)
                    .min(chars.len().saturating_sub(window))
            })
            .collect::<std::collections::BTreeSet<_>>();
        candidates
            .into_iter()
            .max_by_key(|start| {
                let window = chars[*start..(*start + window).min(chars.len())]
                    .iter()
                    .collect::<String>()
                    .to_lowercase();
                let matched = terms
                    .iter()
                    .filter(|term| window.contains(term.as_str()))
                    .map(|term| term.chars().count())
                    .sum::<usize>();
                let occurrences = terms
                    .iter()
                    .map(|term| window.matches(term.as_str()).take(3).count())
                    .sum::<usize>();
                (matched, occurrences, *start)
            })
            .unwrap_or(0)
    };
    let word_start = |start: usize| {
        chars[start.saturating_sub(80)..=start]
            .iter()
            .rposition(|ch| *ch == ' ')
            .map(|at| start.saturating_sub(80) + at + 1)
            .unwrap_or(start)
    };
    let word_end = |start: usize, window: usize| {
        let end = (start + window).min(chars.len());
        if end < chars.len()
            && let Some(at) = chars[start..end].iter().rposition(|ch| *ch == ' ')
            && at > window / 2
        {
            return start + at;
        }
        end
    };
    let start = best_start(budget);
    if start > budget / 2 && budget >= 600 {
        let lead_budget = budget / 3;
        let lead_end = chars[..lead_budget]
            .iter()
            .rposition(|ch| *ch == ' ')
            .filter(|at| *at >= lead_budget / 2)
            .unwrap_or(lead_budget);
        let focus_budget = budget - lead_budget - 4;
        let focus_start = word_start(best_start(focus_budget));
        let focus_end = word_end(focus_start, focus_budget);
        let mut result = format!(
            "{} … {}",
            chars[..lead_end].iter().collect::<String>().trim(),
            chars[focus_start..focus_end]
                .iter()
                .collect::<String>()
                .trim()
        );
        if focus_end < chars.len() {
            result.push('…');
        }
        return bounded(&result, budget, false);
    }
    let start = if start > 0 { word_start(start) } else { start };
    let end = word_end(start, budget);
    let mut result = chars[start..end]
        .iter()
        .collect::<String>()
        .trim()
        .to_owned();
    if start > 0 {
        result.insert(0, '…');
    }
    if end < chars.len() {
        result.push('…');
    }
    bounded(&result, budget, false)
}

mod worker;
pub use worker::{WorkBudget, atomic_json, backfill, status_json};

#[cfg(test)]
mod tests {
    use super::{Mode, SearchRequest, passage, search};
    use crate::Hit;
    use llm_history_store::Store;
    use llm_history_types::Harness;
    use llm_history_types::query::Filter;
    use serde_json::Value;
    use serde_json::json;

    #[test]
    fn guardian_hits_show_what_was_reviewed_and_the_verdict() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("codex");
        let path = root.join("sessions/guardian.jsonl");
        std::fs::create_dir_all(path.parent().unwrap())?;
        let records = [
            json!({"type":"session_meta","payload":{"id":"guardian","source":{"subagent":{"other":"guardian"}}}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"The following is the Codex agent history. The agent asks to run git push --force origin dev."}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":"{\"risk_level\":\"high\",\"outcome\":\"deny\",\"rationale\":\"token sk-abcdefghijklmnopqrstuv\"}"}}),
        ];
        std::fs::write(
            &path,
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
        let store = Store::open(&dir.path().join("history.duckdb"), false)?;
        store.add_profile("work", Harness::Codex, &root, None, None)?;
        let updated = crate::fixtures::capture(&store, &[], false)?;
        assert!(updated.errors.is_empty(), "{updated:?}");
        store.create_fts()?;
        let mut request = SearchRequest {
            query: "force push origin".into(),
            filter: Filter {
                kind: Some("guardian".into()),
                ..Filter::default()
            },
            ..SearchRequest::default()
        };
        let response = search(|| Ok(&store), &request, None, None)?;
        assert_eq!(response.results.len(), 1);
        let hit = &response.results[0];
        assert_eq!(hit.message.role, "user");
        assert!(hit.message.text.contains("git push --force"), "{hit:?}");
        let verdict = hit.verdict.as_deref().unwrap();
        assert!(verdict.contains("\"outcome\":\"deny\""), "{verdict}");
        assert!(!verdict.contains("sk-abc"), "{verdict}");
        // Other sessions keep their prompt-to-answer swap, with no verdict.
        request.filter.kind = Some("interactive".into());
        assert!(
            search(|| Ok(&store), &request, None, None)?
                .results
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn guardian_search_covers_the_reviewed_action_not_the_quoted_transcript() -> anyhow::Result<()>
    {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("codex");
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions)?;
        let reviewed = "The following is the Codex agent history whose request action you are assessing.\n\
            >>> TRANSCRIPT START\n[1] user: please rebase the feature branch\n>>> TRANSCRIPT END\n\
            Reviewed Codex session id: parent\n\
            The Codex agent has requested the following action:\n\
            >>> APPROVAL REQUEST START\nPlanned action JSON:\n\
            {\n  \"command\": [\"git\", \"push\", \"--force\"]\n}\n\
            >>> APPROVAL REQUEST END\n";
        // Reviews of native tool calls have no block, so the whole prompt is searched.
        let unmarked = ">>> TRANSCRIPT START\n[1] user: polish the widget\n>>> TRANSCRIPT END\n";
        for (name, prompt) in [("reviewed", reviewed), ("unmarked", unmarked)] {
            let records = [
                json!({"type":"session_meta","payload":{"id":name,"source":{"subagent":{"other":"guardian"}}}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]}}),
            ];
            std::fs::write(
                sessions.join(format!("{name}.jsonl")),
                records.iter().map(|r| format!("{r}\n")).collect::<String>(),
            )?;
        }
        let store = Store::open(&dir.path().join("history.duckdb"), false)?;
        store.add_profile("work", Harness::Codex, &root, None, None)?;
        let updated = crate::fixtures::capture(&store, &[], false)?;
        assert!(updated.errors.is_empty(), "{updated:?}");
        store.create_fts()?;
        let hits = |query: &str, full: bool| {
            search(
                || Ok(&store),
                &SearchRequest {
                    query: query.into(),
                    filter: Filter {
                        kind: Some("guardian".into()),
                        ..Filter::default()
                    },
                    full,
                    ..SearchRequest::default()
                },
                None,
                None,
            )
            .map(|response| response.results)
        };
        assert!(hits("rebase", false)?.is_empty());
        let action = hits("force", false)?;
        assert_eq!(action.len(), 1);
        assert!(action[0].message.text.contains("--force"), "{action:?}");
        assert!(!action[0].message.text.contains("rebase"), "{action:?}");
        // The prompt itself is untouched, for --full and show.
        assert!(hits("force", true)?[0].message.text.contains("rebase"));
        assert_eq!(hits("widget", false)?.len(), 1);
        Ok(())
    }

    type ExactSession<'a> = (&'a str, Value, Vec<(&'a str, String)>);

    /// A store over Codex sessions of (name, source, messages), oldest first. Exact search
    /// needs no FTS index, so none is built.
    fn exact_store(dir: &std::path::Path, sessions: &[ExactSession]) -> anyhow::Result<Store> {
        let root = dir.join("codex");
        std::fs::create_dir_all(root.join("sessions"))?;
        let mut minute = 0;
        for (name, source, messages) in sessions {
            let mut records = vec![
                json!({"type":"session_meta","payload":{"id":name,"source":source}}).to_string(),
            ];
            for (role, text) in messages {
                minute += 1;
                records.push(
                    json!({
                        "type":"response_item",
                        "timestamp":format!("2026-01-01T{:02}:{:02}:00Z", minute / 60, minute % 60),
                        "payload":{"type":"message","role":role,"phase":"final_answer",
                            "content":[{"type":"input_text","text":text}]}
                    })
                    .to_string(),
                );
            }
            std::fs::write(
                root.join(format!("sessions/{name}.jsonl")),
                records.join("\n") + "\n",
            )?;
        }
        let store = Store::open(&dir.join("history.duckdb"), false)?;
        store.add_profile("work", Harness::Codex, &root, None, None)?;
        let updated = crate::fixtures::capture(&store, &[], false)?;
        assert!(updated.errors.is_empty(), "{updated:?}");
        Ok(store)
    }

    fn exact(store: &Store, query: &str) -> anyhow::Result<Vec<Hit>> {
        Ok(search(
            || Ok(store),
            &SearchRequest {
                query: query.into(),
                mode: Mode::Exact,
                limit: 20,
                per_session: 20,
                ..SearchRequest::default()
            },
            None,
            None,
        )?
        .results)
    }

    #[test]
    fn exact_search_matches_the_literal_string_only() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let user = |text: &str| ("user", text.to_owned());
        let store = exact_store(
            dir.path(),
            &[(
                "s",
                json!("cli"),
                vec![
                    user("please run git push --force now"),
                    user("git force push"),
                    user("uv tool install --force"),
                    user("call render(frame) here"),
                    user("about 100 percent done"),
                ],
            )],
        )?;
        let texts = |query: &str| -> anyhow::Result<Vec<String>> {
            Ok(exact(&store, query)?
                .into_iter()
                .map(|hit| hit.message.text)
                .collect())
        };
        let found = vec!["please run git push --force now".to_owned()];
        assert_eq!(texts("git push --force")?, found);
        // Lexical search would match any of these words.
        store.create_fts()?;
        assert_eq!(texts("git push --force")?, found);
        assert_eq!(texts("GIT PUSH --Force")?, found);
        assert_eq!(texts("render(frame)")?.len(), 1);
        // Punctuation counts, and LIKE wildcards are plain characters.
        assert!(texts("render frame")?.is_empty());
        assert!(texts("100%")?.is_empty());
        assert!(texts("git_push")?.is_empty());
        Ok(())
    }

    #[test]
    fn exact_hits_are_newest_first_and_keep_the_matched_message() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = exact_store(
            dir.path(),
            &[(
                "s",
                json!("cli"),
                vec![
                    ("user", "needle first".to_owned()),
                    ("assistant", "an answer without the word".to_owned()),
                    ("user", "needle second".to_owned()),
                ],
            )],
        )?;
        let texts: Vec<_> = exact(&store, "needle")?
            .into_iter()
            .map(|hit| hit.message.text)
            .collect();
        assert_eq!(texts, ["needle second", "needle first"]);
        Ok(())
    }

    #[test]
    fn exact_search_keeps_older_sessions_past_a_busy_one() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let busy = (0..60).map(|i| ("user", format!("needle {i}"))).collect();
        let store = exact_store(
            dir.path(),
            &[
                (
                    "quiet",
                    json!("cli"),
                    vec![("user", "needle old".to_owned())],
                ),
                ("busy", json!("cli"), busy),
            ],
        )?;
        let response = search(
            || Ok(&store),
            &SearchRequest {
                query: "needle".into(),
                mode: Mode::Exact,
                ..SearchRequest::default()
            },
            None,
            None,
        )?;
        assert_eq!(response.results.len(), 3);
        assert_eq!(response.results[2].message.text, "needle old");
        // The total ignores both --limit and --per-session.
        assert_eq!(response.matched, Some(61));
        Ok(())
    }

    #[test]
    fn exact_snippets_show_the_phrase_in_a_long_message() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let words = "force push git ".repeat(40);
        let text = format!("{words}git push --force {words}");
        let store = exact_store(dir.path(), &[("s", json!("cli"), vec![("user", text)])])?;
        let hits = search(
            || Ok(&store),
            &SearchRequest {
                query: "git push --force".into(),
                mode: Mode::Exact,
                snippet_chars: 200,
                ..SearchRequest::default()
            },
            None,
            None,
        )?
        .results;
        assert!(
            hits[0].message.text.contains("git push --force"),
            "{hits:?}"
        );
        Ok(())
    }

    #[test]
    fn exact_guardian_search_covers_the_action_and_keeps_the_verdict() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let prompt = ">>> TRANSCRIPT START\n[1] user: mentions git rebase\n>>> TRANSCRIPT END\n\
            >>> APPROVAL REQUEST START\n{\"command\":\"git push --force\"}\n\
            >>> APPROVAL REQUEST END\n";
        let store = exact_store(
            dir.path(),
            &[(
                "review",
                json!({"subagent":{"other":"guardian"}}),
                vec![
                    ("user", prompt.to_owned()),
                    ("assistant", "{\"outcome\":\"allow\"}".to_owned()),
                ],
            )],
        )?;
        let request = |query: &str| SearchRequest {
            query: query.into(),
            mode: Mode::Exact,
            filter: Filter {
                kind: Some("guardian".into()),
                ..Filter::default()
            },
            ..SearchRequest::default()
        };
        let outside = search(|| Ok(&store), &request("git rebase"), None, None)?;
        assert!(outside.results.is_empty());
        assert_eq!(outside.matched, Some(0));
        let response = search(|| Ok(&store), &request("git push --force"), None, None)?;
        assert_eq!(response.matched, Some(1));
        let hits = response.results;
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].message.text.contains("git push --force"));
        assert!(hits[0].verdict.as_deref().unwrap().contains("allow"));
        // Limiting the search to user messages still reports the verdict.
        let mut prompts = request("FORCE git");
        prompts.mode = Mode::Words;
        prompts.filter.role = Some("user".into());
        let hits = search(|| Ok(&store), &prompts, None, None)?.results;
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].verdict.as_deref().unwrap().contains("allow"));
        // The default swap would have replaced this hit with an answer lacking the string.
        assert!(exact(&store, "git push --force")?.is_empty());
        Ok(())
    }

    #[test]
    fn words_search_needs_every_word_in_any_order() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let user = |text: &str| ("user", text.to_owned());
        let store = exact_store(
            dir.path(),
            &[(
                "s",
                json!("cli"),
                vec![
                    user("please run git push --force now"),
                    user("git force push"),
                    user("uv tool install --force"),
                ],
            )],
        )?;
        let texts = |query: &str| -> anyhow::Result<Vec<String>> {
            let request = SearchRequest {
                query: query.into(),
                mode: Mode::Words,
                limit: 20,
                per_session: 20,
                ..SearchRequest::default()
            };
            Ok(search(|| Ok(&store), &request, None, None)?
                .results
                .into_iter()
                .map(|hit| hit.message.text)
                .collect())
        };
        // Newest first, whatever order the words were typed in.
        let both = ["git force push", "please run git push --force now"];
        assert_eq!(texts("FORCE git")?, both);
        assert_eq!(texts("git git")?, both);
        // Each word is a literal string, so a stem finds its longer forms.
        assert_eq!(texts("forc pus")?, both);
        assert_eq!(texts("--force")?.len(), 2);
        assert!(texts("git install")?.is_empty());
        assert!(super::check_rerank(Mode::Words, true).is_err());
        Ok(())
    }

    #[test]
    fn guardian_reviews_cannot_be_searched_semantically() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("history.duckdb"), false)?;
        let request = SearchRequest {
            query: "force push".into(),
            mode: Mode::Semantic,
            filter: Filter {
                kind: Some("guardian".into()),
                ..Filter::default()
            },
            ..SearchRequest::default()
        };
        let error = search(|| Ok(&store), &request, None, None).unwrap_err();
        assert!(error.to_string().contains("not embedded"), "{error}");
        Ok(())
    }

    #[test]
    fn snippets_and_reranker_inputs_include_the_relevant_passage() {
        let text = format!(
            "Deployment checklist. {} Rollback deployment: stop the service and restore the snapshot. {}",
            "Background details. ".repeat(300),
            "More details. ".repeat(300)
        );
        for budget in [600, 3200] {
            let selected = passage(&text, "how do we rollback deployment", budget, false, false);
            assert!(
                selected.contains("stop the service and restore the snapshot"),
                "{selected}"
            );
            assert!(selected.chars().count() <= budget);
        }
        assert_eq!(passage(&text, "deployment", usize::MAX, false, true), text);
    }
}

#[cfg(test)]
#[path = "../../../fixtures/prepared_transcripts.rs"]
mod fixtures;

//! Acceptance tests use History, canonical public contracts and synthetic files.
use anyhow::Result;
use llm_history_app::api::{ingestion::UpdateRequest, query::MessageQuery, sql::SqlRequest};
use llm_history_app::{History, HistoryOptions, ProfileRegistration};
use llm_history_models::{
    Embedder, ModelProfile, Reranker, TokenCounter, gemma_profile, qwen_reranker_profile,
};
use llm_history_search::WorkBudget;
use llm_history_store::archives::{restore_archive, verify_archive};
use llm_history_types::Harness;
use llm_history_types::{
    archive::RawFilter,
    query::{Filter, Message, Mode, SearchRequest, Selection},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs};
#[path = "../../../fixtures/contracts/support.rs"]
mod support;
use support::Fixture;

fn setup() -> Result<(Fixture, History)> {
    let fixture = Fixture::new()?;
    let history = History::new(fixture.root().join("history.duckdb"));
    assert!(!history.database().exists());
    for p in fixture.manifest["profiles"].as_array().unwrap() {
        history.register_profile(&ProfileRegistration {
            name: p["name"].as_str().unwrap().into(),
            harness: p["harness"].as_str().unwrap().parse()?,
            root: fixture.root().join(p["root"].as_str().unwrap()),
            account: None,
            backend: None,
        })?;
    }
    Ok((fixture, history))
}
fn query(history: &History, sql: &str) -> Result<Value> {
    Ok(history.sql(&SqlRequest {
        query: sql.into(),
        max_output_bytes: 100_000,
        ..Default::default()
    })?["rows"]
        .clone())
}

#[test]
fn sql_exposes_bundled_hashes_on_a_read_only_reopen() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let history = History::new(dir.path().join("history.duckdb"));
    history.create()?;
    assert_eq!(
        query(
            &history,
            "SELECT lower(hex(llm_history_blake3(from_hex('')))) AS digest,
                    llm_history_xxh64(from_hex('')) AS fingerprint,
                    typeof(fingerprint) AS fingerprint_type,
                    llm_history_blake3(NULL::BLOB) IS NULL AS null_digest,
                    llm_history_xxh64(NULL::BLOB) IS NULL AS null_fingerprint",
        )?,
        json!([{
            "digest":"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            "fingerprint":0xef46_db37_51d8_e999_u64,
            "fingerprint_type":"UBIGINT",
            "null_digest":true,
            "null_fingerprint":true
        }])
    );
    Ok(())
}

fn all_messages(history: &History) -> Result<Vec<Message>> {
    history.messages(&MessageQuery {
        filter: Filter {
            include_subagents: true,
            include_agent_protocol: true,
            ..Default::default()
        },
        ..Default::default()
    })
}
fn identity(messages: &[Message]) -> BTreeMap<String, (String, String)> {
    messages
        .iter()
        .map(|m| (m.id.clone(), (m.session_id.clone(), m.text.clone())))
        .collect()
}
fn assert_usage(history: &History, expected: &Value) -> Result<()> {
    let periods = history.usage(&Filter::default(), "UTC", false, true, 100)?;
    let usage = serde_json::to_value(periods.iter().find(|p| p.profile == "claude").unwrap())?;
    for (key, value) in expected.as_object().unwrap() {
        assert_eq!(&usage[key], value, "usage {key}");
    }
    Ok(())
}
fn assert_raw(fixture: &Fixture, history: &History, appended: bool) -> Result<()> {
    for source in history.raw_sources(&RawFilter {
        latest: true,
        ..Default::default()
    })? {
        let p = fixture.profile(&source.profile);
        let mut expected = fs::read(fixture.inputs.join(p["input"].as_str().unwrap()))?;
        if appended && source.profile == "claude" {
            expected.extend(fs::read(fixture.inputs.join("claude-append.jsonl"))?);
        }
        let mut actual = Vec::new();
        assert_eq!(
            history.export_raw(source.id, Some(source.generation), &mut actual)?,
            expected.len() as u64
        );
        assert_eq!(actual, expected);
    }
    assert_eq!(
        history.verify_raw(&RawFilter::default(), false, false)?["verified"],
        true
    );
    Ok(())
}

#[test]
fn public_scenario_preserves_profiles_tools_usage_ids_and_bytes_through_replay() -> Result<()> {
    let (fixture, mut history) = setup()?;
    let mut previous = BTreeMap::new();
    let mut appended = false;
    for step in fixture.manifest["steps"].as_array().unwrap() {
        match step["operation"].as_str().unwrap() {
            "update" => {
                let receipt = history.update(&UpdateRequest::default())?.receipt;
                assert!(receipt.errors.is_empty(), "{receipt:?}");
                match step["expect"].as_str().unwrap() {
                    "initial" => {
                        let sessions = history.sessions(&Filter::default(), 100, false)?;
                        assert_eq!(sessions.len(), fixture.expected["initial"]["sessions"]);
                        assert_eq!(
                            sessions
                                .iter()
                                .map(|s| &s.id)
                                .collect::<std::collections::BTreeSet<_>>()
                                .len(),
                            3
                        );
                        let rows = query(
                            &history,
                            "SELECT native_call_id,tool_name,arguments_json,output_preview,has_result FROM history_tool_activity ORDER BY native_call_id",
                        )?;
                        assert_eq!(rows, fixture.expected["tools"]);
                        assert_eq!(
                            query(
                                &history,
                                "SELECT count(*) AS n FROM history_event_relations"
                            )?[0]["n"],
                            3
                        );
                        assert_eq!(
                            all_messages(&history)?.len(),
                            fixture.expected["initial"]["messages"]
                        );
                    }
                    "appended" => assert_eq!(
                        all_messages(&history)?.len(),
                        fixture.expected["appended"]["messages"]
                    ),
                    "missing" => {
                        assert_eq!(receipt.missing, fixture.expected["missing"]["sources"])
                    }
                    other => panic!("unknown observation {other}"),
                }
                let current = identity(&all_messages(&history)?);
                for (id, message) in &previous {
                    assert_eq!(current.get(id), Some(message), "retained message changed");
                }
                previous = current;
                let claude_text = all_messages(&history)?
                    .into_iter()
                    .filter(|m| m.profile == "claude")
                    .map(|m| m.text)
                    .collect::<Vec<_>>();
                assert_eq!(
                    json!(claude_text),
                    fixture.expected[if appended { "appended" } else { "initial" }]["claude_text"]
                );
                assert_usage(&history, &fixture.expected["initial"]["claude_usage"])?;
                assert_raw(&fixture, &history, appended)?;
            }
            "append" => {
                fixture.append(
                    step["profile"].as_str().unwrap(),
                    step["input"].as_str().unwrap(),
                )?;
                appended = true;
            }
            "reopen" => {
                history = History::new(history.database());
            }
            "remove_sources" => {
                for p in fixture.manifest["profiles"].as_array().unwrap() {
                    fs::remove_dir_all(fixture.root().join(p["root"].as_str().unwrap()))?;
                }
            }
            "reprocess" => {
                let before = fs::read(history.database())?;
                let dry = history.reprocess(&Selection::default(), &[], true)?;
                assert!(dry.errors.is_empty());
                assert_eq!(fs::read(history.database())?, before);
                let receipt = history.reprocess(&Selection::default(), &[], false)?;
                assert!(receipt.errors.is_empty(), "{receipt:?}");
                assert_eq!(receipt.sources, fixture.expected["replayed"]["sources"]);
                assert_eq!(identity(&all_messages(&history)?), previous);
                assert_usage(&history, &fixture.expected["initial"]["claude_usage"])?;
                assert_raw(&fixture, &history, true)?;
            }
            other => panic!("unknown scenario operation {other}"),
        }
    }
    Ok(())
}

#[test]
fn public_queries_distinguish_excerpts_context_and_profile_scoped_identity() -> Result<()> {
    let (fixture, history) = setup()?;
    history.update(&UpdateRequest::default())?;
    let filter = Filter {
        profiles: vec!["personal".into()],
        ..Default::default()
    };
    let messages = history.messages(&MessageQuery {
        filter: filter.clone(),
        limit: 1,
        after: None,
    })?;
    assert_eq!(messages.len(), 1);
    let next = history.messages(&MessageQuery {
        filter: filter.clone(),
        limit: 1,
        after: Some(messages[0].id.clone()),
    })?;
    assert_eq!(next.len(), 1);
    assert_ne!(next[0].id, messages[0].id);
    let request = SearchRequest {
        query: "repair lunar cache".into(),
        mode: Mode::Exact,
        filter: filter.clone(),
        snippet_chars: 12,
        ..Default::default()
    };
    let response = history.search(&request, None, None)?;
    assert!(response.details.get("eligible_documents").is_none());
    let explained = history.search(
        &SearchRequest {
            explain: true,
            ..request.clone()
        },
        None,
        None,
    )?;
    assert_eq!(explained.matched, response.matched);
    assert_eq!(
        explained.results[0].message.id,
        response.results[0].message.id
    );
    assert_eq!(explained.details["eligible_documents"], 2);
    assert_eq!(response.matched, Some(1));
    assert_eq!(response.results[0].message.id, messages[0].id);
    assert!(response.results[0].message.text.chars().count() <= 12);
    let context = history.context(messages[0].message_no, 1, 1, &["personal".into()])?;
    assert_eq!(
        context.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        [
            "repair lunar cache",
            "Read the retained archive to restore the lunar cache."
        ]
    );
    assert!(
        history
            .context(messages[0].message_no, 1, 1, &["work".into()])
            .is_err()
    );
    let ranked = history.search(
        &SearchRequest {
            query: "repair".into(),
            filter,
            ..Default::default()
        },
        None,
        None,
    )?;
    assert_eq!(
        ranked.results[0].message.id, next[0].id,
        "ranked search keeps current answer selection"
    );
    assert_eq!(history.projects(&Filter::default(), 100, false)?.len(), 3);
    let raw = history.raw_sources(&RawFilter {
        selector: Some("shared-native".into()),
        latest: true,
        ..Default::default()
    })?;
    assert_eq!(raw.len(), 2, "Codex native IDs collide across profiles");
    // Retaining a History handle holds no reader/writer lock, including across clones.
    let second = history.clone();
    let before = second.profiles(&["personal".into()], false)?.remove(0);
    let disabled = second.set_profile_enabled("personal", false)?;
    assert_eq!(disabled.id, before.id);
    assert!(!disabled.enabled);
    history.set_profile_enabled("personal", true)?;
    let moved = fixture.root().join("moved");
    fs::rename(fixture.root().join("personal"), &moved)?;
    assert_eq!(history.relocate_profile("personal", &moved)?.id, before.id);
    assert!(
        history
            .update(&UpdateRequest::default())?
            .receipt
            .errors
            .is_empty()
    );
    assert_eq!(
        history.context(messages[0].message_no, 0, 0, &[])?[0].text,
        "repair lunar cache"
    );
    Ok(())
}

#[test]
fn handle_construction_and_failed_reads_do_not_create_a_database_or_discover_profiles() -> Result<()>
{
    let directory = tempfile::tempdir()?;
    let history = History::new(directory.path().join("absent/history.duckdb"));
    assert!(history.sessions(&Filter::default(), 10, false).is_err());
    assert!(!history.database().parent().unwrap().exists());
    history.initialize()?;
    assert!(history.profiles(&[], false)?.is_empty());
    assert!(
        history
            .messages(&MessageQuery {
                limit: 0,
                ..Default::default()
            })
            .is_err()
    );
    Ok(())
}

#[test]
fn default_candidates_and_refresh_are_explicit_and_scoped() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut options = HistoryOptions::new(fixture.root().join("history.duckdb"));
    options.default_profiles.push(ProfileRegistration {
        name: "personal".into(),
        harness: Harness::Codex,
        root: fixture.root().join("personal"),
        account: None,
        backend: None,
    });
    let history = History::with_options(options);
    history.initialize()?;
    assert_eq!(history.profiles(&[], false)?.len(), 1);
    let receipts = history.refresh(&["personal".into()], Some("codex"), false)?;
    assert_eq!(receipts.len(), 1);
    assert!(receipts[0].errors.is_empty());
    assert_eq!(receipts[0].imported, 1);
    assert!(
        history
            .refresh(&["personal".into()], Some("codex"), false)?
            .is_empty()
    );
    assert_eq!(
        history
            .refresh(&["personal".into()], Some("codex"), true)?
            .len(),
        1
    );
    assert_eq!(history.messages(&MessageQuery::default())?.len(), 2);
    assert!(history.refresh(&[], Some("misspelled"), false).is_err());
    Ok(())
}

#[test]
fn refresh_bails_out_unchanged_and_picks_up_immediate_changes() -> Result<()> {
    use std::io::Write;

    let (fixture, history) = setup()?;
    let profile = ["personal".to_owned()];
    assert_eq!(history.refresh(&profile, None, false)?[0].imported, 1);

    let mut options = HistoryOptions::new(history.database());
    options.lock_timeout = std::time::Duration::ZERO;
    let history = History::with_options(options);
    // An unchanged search must not try to take the writer lock.
    let reader = llm_history_store::Store::open(history.database(), true)?;
    assert!(history.refresh(&profile, None, false)?.is_empty());
    drop(reader);

    let records =
        llm_history_core::fixture_record(Harness::Codex, "shared-native", "immediate append");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(fixture.source("personal"))?,
        "{}",
        records[1]
    )?;
    let refreshed = history.refresh(&profile, None, false)?;
    assert_eq!(refreshed[0].appended, 1);
    assert_eq!(refreshed[0].records, 1);
    assert!(history.refresh(&profile, None, false)?.is_empty());

    fs::write(
        fixture.root().join("personal/.codex-global-state.json"),
        "{}",
    )?;
    assert_eq!(history.refresh(&profile, None, false)?[0].records, 0);
    assert!(history.refresh(&profile, None, false)?.is_empty());

    fs::remove_file(fixture.source("personal"))?;
    assert_eq!(history.refresh(&profile, None, false)?[0].missing, 1);
    assert!(history.refresh(&profile, None, false)?.is_empty());
    Ok(())
}

#[test]
fn a_bad_source_reports_partial_success_and_can_be_retried() -> Result<()> {
    let (fixture, history) = setup()?;
    let bad = fixture.root().join("work/sessions/broken.jsonl.zst");
    fs::write(&bad, b"synthetic invalid compressed transcript")?;
    let outcome = history.update(&UpdateRequest::default())?;
    assert_eq!(outcome.receipt.errors.len(), 1);
    assert_eq!(all_messages(&history)?.len(), 7);
    assert_raw(&fixture, &history, false)?;
    fs::remove_file(bad)?;
    let retry = history.update(&UpdateRequest::default())?;
    assert!(retry.receipt.errors.is_empty());
    assert_eq!(all_messages(&history)?.len(), 7);
    Ok(())
}

#[test]
fn snapshots_restore_and_compaction_keep_public_history_and_identity() -> Result<()> {
    let (fixture, history) = setup()?;
    history.update(&UpdateRequest::default())?;
    let before = identity(&all_messages(&history)?);
    let database_identity = history.status(&Selection::default())?["identity"].clone();
    let snapshot = fixture.root().join("snapshot.duckdb");
    history.snapshot(&snapshot)?;
    assert_eq!(
        verify_archive(&snapshot)?.identity,
        database_identity.as_str().unwrap()
    );
    fixture.append("claude", "claude-append.jsonl")?;
    history.update(&UpdateRequest::default())?;
    assert_eq!(all_messages(&history)?.len(), 8);
    let restored = restore_archive(&snapshot, history.database(), true)?;
    assert!(restored.archived_previous.unwrap().is_file());
    assert_eq!(identity(&all_messages(&history)?), before);
    history.compact()?;
    assert_eq!(
        history.status(&Selection::default())?["identity"],
        database_identity
    );
    assert_eq!(identity(&all_messages(&history)?), before);
    assert_raw(&fixture, &history, false)?;
    Ok(())
}

struct Fake {
    history: History,
    profile: ModelProfile,
    calls: usize,
}
impl TokenCounter for Fake {
    fn token_count(&mut self, text: &str) -> Result<usize> {
        Ok(text.chars().count())
    }
}
impl Embedder for Fake {
    fn profile(&self) -> &ModelProfile {
        &self.profile
    }
    fn embed(&mut self, texts: &[String], _query: bool) -> Result<Vec<Vec<f32>>> {
        self.history.index(false)?;
        self.calls += texts.len();
        Ok(texts
            .iter()
            .map(|text| {
                if text.contains("lunar") {
                    vec![1., 0.]
                } else {
                    vec![0., 1.]
                }
            })
            .collect())
    }
}
#[test]
fn public_backfill_accepts_fake_inference_without_retaining_a_database_lock() -> Result<()> {
    let (fixture, history) = setup()?;
    history.update(&UpdateRequest::default())?;
    let mut profile = gemma_profile(768)?;
    profile.id = "public-fixture-two-dimensional".into();
    profile.dimensions = 2;
    profile.document_prefix.clear();
    let mut fake = Fake {
        history: history.clone(),
        profile,
        calls: 0,
    };
    let queue = fixture.root().join("queue");
    let budget = WorkBudget {
        profiles: vec!["personal".into()],
        ..Default::default()
    };
    assert_eq!(history.backfill(&queue, &mut fake, &budget)?.merged, 2);
    assert_eq!(fake.calls, 2);
    assert_eq!(history.backfill(&queue, &mut fake, &budget)?.merged, 0);
    assert_eq!(fake.calls, 2, "resume repeats no inference");
    let result = history.search(
        &SearchRequest {
            query: "lunar".into(),
            mode: Mode::Semantic,
            filter: Filter {
                profiles: vec!["personal".into()],
                ..Default::default()
            },
            ..Default::default()
        },
        Some(&mut fake),
        None,
    )?;
    assert!(!result.results.is_empty());
    assert_eq!(
        history.coverage(&fake.profile.id, &["personal".into()])?["current"],
        json!(2)
    );
    Ok(())
}

struct WriteCheckingReranker {
    history: History,
    profile: ModelProfile,
    calls: usize,
}
impl Reranker for WriteCheckingReranker {
    fn profile(&self) -> &ModelProfile {
        &self.profile
    }

    fn rerank(&mut self, _query: &str, documents: &[String]) -> Result<Vec<(usize, f64)>> {
        self.calls += 1;
        self.history.index(false)?;
        Ok(documents.iter().enumerate().map(|(i, _)| (i, 1.)).collect())
    }
}

#[test]
fn foreground_search_releases_the_database_before_model_work() -> Result<()> {
    let (fixture, initialized) = setup()?;
    initialized.update(&UpdateRequest::default())?;
    let mut options = HistoryOptions::new(fixture.root().join("history.duckdb"));
    options.lock_timeout = std::time::Duration::ZERO;
    let history = History::with_options(options);
    let mut embedder = Fake {
        history: history.clone(),
        profile: gemma_profile(768)?,
        calls: 0,
    };
    let mut reranker = WriteCheckingReranker {
        history: history.clone(),
        profile: qwen_reranker_profile(),
        calls: 0,
    };
    let result = history.search(
        &SearchRequest {
            query: "lunar".into(),
            mode: Mode::Hybrid,
            ..Default::default()
        },
        Some(&mut embedder),
        Some(&mut reranker),
    )?;
    assert_eq!(embedder.calls, 1);
    assert_eq!(reranker.calls, 1);
    assert!(result.results.iter().any(|hit| hit.rerank_score.is_some()));
    Ok(())
}

#[test]
fn automatic_refresh_skips_an_in_flight_import_and_committed_rows_remain_readable() -> Result<()> {
    let (_fixture, history) = setup()?;
    history.update(&UpdateRequest::default())?;
    let mut options = HistoryOptions::new(history.database());
    options.lock_timeout = std::time::Duration::ZERO;
    let history = History::with_options(options);
    let lock =
        llm_history_store::Store::lock_ingestion(history.database(), std::time::Duration::ZERO)?;
    assert!(history.refresh(&[], None, false)?.is_empty());
    assert!(!all_messages(&history)?.is_empty());
    assert!(
        history
            .update(&UpdateRequest::default())
            .unwrap_err()
            .is::<llm_history_types::storage::DatabaseBusy>()
    );
    drop(lock);
    assert_eq!(
        history.update(&UpdateRequest::default())?.receipt.records,
        0
    );
    Ok(())
}

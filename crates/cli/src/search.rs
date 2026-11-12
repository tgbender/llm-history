//! Searching history and finding similar messages.
use super::*;
use llm_history_types::query::Mode;
use llm_history_types::query::SearchRequest;

pub(crate) fn search(ctx: &Ctx, args: Search) -> Result<()> {
    llm_history_search::check_rerank(args.mode, args.rerank && !args.no_rerank)?;
    // Reject a bad date or zone before refreshing or starting a model.
    let since = time_bound(&args.since, &args.timezone)?;
    let until = time_bound(&args.until, &args.timezone)?;
    let refresh = if args.no_refresh {
        Value::Null
    } else {
        refresh(
            &ctx.db,
            &ctx.cache,
            &args.profile,
            args.source.as_deref(),
            args.force_refresh,
        )?
    };
    let mut embedding = if matches!(args.mode, Mode::Semantic | Mode::Hybrid) {
        Some(engine(&ctx.cache, &args.inference, false)?)
    } else if args.mode == Mode::Auto {
        engine(&ctx.cache, &args.inference, false).ok()
    } else {
        None
    };
    let mut reranking = if args.rerank && !args.no_rerank {
        Some(engine(&ctx.cache, &args.inference, true)?)
    } else {
        None
    };
    let scope = match args.role.as_str() {
        "user" => "user messages",
        "assistant" => "assistant answers",
        _ => "messages",
    };
    let s = ctx.history();
    let request = SearchRequest {
        query: args.query,
        mode: args.mode,
        filter: Filter {
            profiles: args.profile.clone(),
            source: args.source.clone(),
            since,
            until,
            role: (args.role != "all").then_some(args.role),
            threads: args.thread,
            cwd: args.cwd.map(expand_home),
            under: args.under.map(expand_home),
            project: args.project,
            include_subagents: !args.no_subagents,
            include_agent_protocol: args.include_agent_protocol,
            kind: args.kind,
        },
        limit: args.limit as usize,
        per_session: args.per_session as usize,
        snippet_chars: args.snippet_chars,
        max_output_chars: args.max_output_chars,
        full: args.full,
        redact: !args.no_redact,
        rerank_depth: args.rerank_depth as usize,
        explain: args.explain,
    };
    let mut response = s.search(
        &request,
        embedding.as_mut().map(Engine::embedding),
        reranking.as_mut().map(Engine::reranking),
    )?;
    if args.explain {
        response.details["refresh"] = refresh;
    }
    let (shown, matched) = (response.results.len(), response.matched);
    if ctx.json {
        emit(
            &serde_json::to_value(crate::output::SearchOutput::new(&response, args.explain))?,
            true,
        )?;
    } else {
        for (i, h) in response.results.iter().enumerate() {
            println!(
                "{}  {}  {}  {}  {}  id={}  session={}\n  {}",
                i + 1,
                h.message.ts,
                h.message.profile,
                if h.message.project.is_empty() {
                    llm_history_core::project_name(&h.message.cwd)
                } else {
                    h.message.project.clone()
                },
                h.message.role,
                h.message.message_no,
                h.message.session_no,
                h.message.text
            );
            if let Some(verdict) = &h.verdict {
                println!("  verdict: {verdict}");
            }
        }
        // Says what was covered, so an empty result isn't mistaken for a stale archive.
        if let Some(searched) = &response.searched {
            let through = searched
                .through
                .as_deref()
                .map_or(String::new(), |at| format!("; archive through {at}"));
            println!(
                "No matches in {} {scope}{through}.",
                thousands(searched.messages)
            );
        }
        if let Some(matched) = matched.filter(|&matched| matched > shown) {
            println!(
                "{shown} of {matched} matches shown; \
                 raise --limit, --per-session or --max-output-chars for more"
            );
        }
        if args.explain {
            eprintln!("{}", response.details);
        }
    }
    if !args.no_background {
        maybe_background(
            &ctx.db,
            &ctx.cache,
            &args.profile,
            args.source.as_deref(),
            &args.inference,
        );
    }
    Ok(())
}

/// A count with commas between thousands.
fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

pub(crate) fn similar(ctx: &Ctx, args: SimilarArgs) -> Result<()> {
    let hits = ctx
        .history()
        .similar(&llm_history_app::api::search::SimilarRequest {
            message_no: args.id,
            dimensions: args.dimensions,
            filter: Filter {
                profiles: args.profile,
                source: args.source,
                project: args.project,
                role: args.role,
                ..Filter::default()
            },
            limit: usize::from(args.limit),
            per_session: usize::from(args.per_session),
            other_sessions: args.other_sessions,
        })?;
    if ctx.json {
        let rows = hits
            .iter()
            .map(crate::output::SimilarRow::from)
            .collect::<Vec<_>>();
        emit(&json!({"results": rows}), true)?;
    } else {
        for hit in hits {
            let m = hit.message;
            println!(
                "{:.4}  {}  {}  {}  {}  id={}\n  {}",
                hit.similarity, m.ts, m.profile, m.project, m.role, m.message_no, m.text
            );
        }
    }
    Ok(())
}

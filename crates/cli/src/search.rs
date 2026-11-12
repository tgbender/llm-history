//! Searching history and finding similar messages.
use super::*;
use llm_history_types::query::{Mode, SearchRequest, Searched, ToolSearchResponse};

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
    let tools = args.scope == "tools";
    let mut embedding = if tools {
        None
    } else if matches!(args.mode, Mode::Semantic | Mode::Hybrid) {
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
    let covered = match (tools, args.role.as_str()) {
        (true, _) => ("tool call", "tool calls"),
        (false, "user") => ("user message", "user messages"),
        (false, "assistant") => ("assistant answer", "assistant answers"),
        _ => ("message", "messages"),
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
            role: (!tools && args.role != "all").then_some(args.role),
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
    if tools {
        print_tool_hits(ctx, &s.search_tools(&request)?, covered)?;
        if !args.no_background {
            maybe_background(
                &ctx.db,
                &ctx.cache,
                &args.profile,
                args.source.as_deref(),
                &args.inference,
            );
        }
        return Ok(());
    }
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
        if let Some(searched) = &response.searched {
            println!("{}", no_matches(searched, covered));
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

/// Says what an empty search covered, so it isn't mistaken for a stale archive.
fn no_matches(searched: &Searched, (one, many): (&str, &str)) -> String {
    let through = searched
        .through
        .as_deref()
        .map_or(String::new(), |at| format!("; archive through {at}"));
    format!(
        "No matches in {} {}{through}.",
        thousands(searched.count),
        if searched.count == 1 { one } else { many }
    )
}

/// Tool calls print like message hits. The ID is the message before the call, for `show`.
fn print_tool_hits(ctx: &Ctx, response: &ToolSearchResponse, covered: (&str, &str)) -> Result<()> {
    let shown = response.results.len();
    if ctx.json {
        let rows = response
            .results
            .iter()
            .map(crate::output::ToolRow::from)
            .collect::<Vec<_>>();
        let mut result = json!({ "results": rows });
        if response.matched > shown {
            result["matched"] = json!(response.matched);
        }
        if let Some(searched) = &response.searched {
            result["searched"] = serde_json::to_value(searched)?;
        }
        return emit(&result, true);
    }
    for (at, call) in response.results.iter().enumerate() {
        let project = if call.project.is_empty() {
            llm_history_core::project_name(&call.cwd)
        } else {
            call.project.clone()
        };
        let id = call
            .message_no
            .map_or(String::new(), |id| format!("  id={id}"));
        println!(
            "{}  {}  {}  {project}  {}{id}  session={}\n  {}",
            at + 1,
            call.ts,
            call.profile,
            call.tool,
            call.session_no,
            call.text
        );
    }
    if let Some(searched) = &response.searched {
        println!("{}", no_matches(searched, covered));
    }
    if response.matched > shown {
        println!(
            "{shown} of {} matches shown; \
             raise --limit, --per-session or --max-output-chars for more",
            response.matched
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

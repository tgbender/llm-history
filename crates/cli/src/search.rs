//! Searching history and showing a message in its conversation.
use super::*;
use llm_history_types::query::Mode;
use llm_history_types::query::SearchRequest;

pub(crate) fn search(ctx: &Ctx, args: Search) -> Result<()> {
    llm_history_search::check_rerank(args.mode, args.rerank && !args.no_rerank)?;
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
    let s = ctx.history();
    let request = SearchRequest {
        query: args.query,
        mode: args.mode,
        filter: Filter {
            profiles: args.profile.clone(),
            source: args.source.clone(),
            since: time_bound(&args.since, &args.timezone)?,
            until: time_bound(&args.until, &args.timezone)?,
            role: args.role,
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
                "{}  {}  {}  {}  {}  id={}\n  {}",
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
                h.message.text
            );
            if let Some(verdict) = &h.verdict {
                println!("  verdict: {verdict}");
            }
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

pub(crate) fn show(ctx: &Ctx, args: ShowArgs) -> Result<()> {
    let ShowArgs {
        id,
        before,
        after,
        profile,
        max_output_chars,
        no_redact,
        full,
    } = args;
    let store = ctx.history();
    let messages = store.context(id, before, after, &profile)?;
    let target = messages
        .iter()
        .position(|m| m.message_no == id)
        .context("message not found")?;
    let header = &messages[target];
    let scope =
        json!({"profile":header.profile,"session_id":header.session_no,"source":header.source});
    let neighbors = messages.len() - 1;
    let target_budget = if neighbors == 0 {
        max_output_chars
    } else {
        max_output_chars.saturating_mul(2) / 3
    };
    let neighbor_budget = max_output_chars.saturating_sub(target_budget) / neighbors.max(1);
    let mut output = Vec::new();
    for (at, mut message) in messages.into_iter().enumerate() {
        let budget = if full {
            usize::MAX
        } else if at == target {
            target_budget
        } else {
            neighbor_budget
        };
        // A guardian prompt ends with the reviewed action, so cutting its end would lose it.
        let action = (message.session_kind == "guardian" && message.text.chars().count() > budget)
            .then(|| llm_history_core::guardian_action(&message.text))
            .flatten()
            .map(|action| format!("[Quoted transcript omitted; --full shows it]\n{action}"));
        message.text = bounded(
            action.as_deref().unwrap_or(&message.text),
            budget,
            !no_redact,
        );
        let ts = message.timestamp_utc.unwrap_or(message.ts);
        if !ctx.json {
            println!(
                "{}  {}  {}  {}  {}/{}  id={}\n  {}",
                if at == target {
                    "*".into()
                } else {
                    format!("{:+}", at as i64 - target as i64)
                },
                ts,
                message.profile,
                message.project,
                message.role,
                message.authorship,
                message.message_no,
                message.text,
            );
        } else {
            output.push(crate::output::ContextRow {
                id: message.message_no,
                role: message.role,
                ts,
                text: message.text,
                target: (at == target).then_some(true),
            });
        }
    }
    if ctx.json {
        let mut result = scope;
        result["messages"] = serde_json::to_value(output)?;
        emit(&result, true)?;
    }
    Ok(())
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

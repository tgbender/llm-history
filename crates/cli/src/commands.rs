//! Profile, import, listing, SQL and maintenance commands.
use super::*;

pub(crate) fn init(ctx: &Ctx) -> Result<()> {
    emit(&ctx.history().initialize()?, ctx.json)
}

pub(crate) fn profile(ctx: &Ctx, command: Profiles) -> Result<()> {
    let history = ctx.history();
    history.create()?;
    let value = match command {
        Profiles::Add {
            name,
            harness,
            root,
            account,
            backend,
        } => serde_json::to_value(history.register_profile(&ProfileRegistration {
            name,
            harness,
            root,
            account,
            backend,
        })?)?,
        Profiles::List => serde_json::to_value(history.profiles(&[], false)?)?,
        Profiles::Relocate { name, root } => {
            serde_json::to_value(history.relocate_profile(&name, &root)?)?
        }
        Profiles::Enable { ref name } | Profiles::Disable { ref name } => serde_json::to_value(
            history.set_profile_enabled(name, matches!(command, Profiles::Enable { .. }))?,
        )?,
    };
    emit(&value, ctx.json)
}

pub(crate) fn update(
    ctx: &Ctx,
    repair: bool,
    profile: Vec<String>,
    source: Option<String>,
    no_background: bool,
    inference: &Inference,
) -> Result<()> {
    let outcome = ctx
        .history()
        .update(&llm_history_app::api::ingestion::UpdateRequest {
            selection: llm_history_types::query::Selection {
                profiles: profile,
                source: source.clone(),
            },
            repair,
        })?;
    emit(&serde_json::to_value(&outcome.receipt)?, ctx.json)?;
    if !no_background {
        maybe_background(
            &ctx.db,
            &ctx.cache,
            &outcome.profiles,
            source.as_deref(),
            inference,
        );
    }
    if !outcome.receipt.errors.is_empty() {
        bail!("some history sources failed to import");
    }
    Ok(())
}

pub(crate) fn status(ctx: &Ctx, selection: ProfileSelection) -> Result<()> {
    let s = ctx.history();
    let profiles = s
        .profiles(&selection.profile, false)?
        .into_iter()
        .filter(|p| {
            selection
                .source
                .as_deref()
                .is_none_or(|source| source == "all" || source == p.harness.key())
        })
        .collect::<Vec<_>>();
    emit(
        &json!({"database":s.status(&llm_history_types::query::Selection { profiles: selection.profile, source: selection.source })?,"profiles":profiles}),
        ctx.json,
    )?;
    Ok(())
}

pub(crate) fn sessions(ctx: &Ctx, args: SessionArgs) -> Result<()> {
    let s = ctx.history();
    let selected = args.selection;
    let sessions = s.sessions(
        &Filter {
            profiles: selected.profile,
            source: selected.source,
            since: time_bound(&selected.since, &selected.timezone)?,
            until: time_bound(&selected.until, &selected.timezone)?,
            include_subagents: args.include_subagents,
            kind: args.kind,
            cwd: args.cwd.map(expand_home),
            under: args.under.map(expand_home),
            project: args.project,
            ..Default::default()
        },
        selected.limit.unwrap_or(30),
        args.oldest_first,
    )?;
    let rows = sessions
        .iter()
        .map(crate::output::SessionRow::from)
        .collect::<Vec<_>>();
    emit(&serde_json::to_value(rows)?, ctx.json)?;
    Ok(())
}

pub(crate) fn projects(ctx: &Ctx, args: ProjectArgs) -> Result<()> {
    let backfill = matches!(args.command, Some(ProjectCommand::Backfill));
    let selection = args.selection;
    let include_inactive = args.include_inactive;
    if backfill && (include_inactive || selection.limit.is_some()) {
        bail!("--all and --limit apply to project listings");
    }
    let store = ctx.history();
    if backfill {
        let receipt = store.refresh_projects(&llm_history_types::query::Selection {
            profiles: selection.profile,
            source: selection.source,
        })?;
        emit(&serde_json::to_value(&receipt)?, ctx.json)?;
        if !receipt.errors.is_empty() {
            bail!("some catalogs failed to refresh");
        }
    } else {
        emit(
            &serde_json::to_value(store.projects(
                &Filter {
                    profiles: selection.profile,
                    source: selection.source,
                    ..Default::default()
                },
                selection.limit.unwrap_or(100).min(10000),
                include_inactive,
            )?)?,
            ctx.json,
        )?;
    }
    Ok(())
}

pub(crate) fn reprocess(ctx: &Ctx, args: ReprocessArgs) -> Result<()> {
    if !ctx.db.is_file() {
        bail!("database does not exist");
    }
    let store = ctx.history();
    let receipt = store.reprocess(
        &llm_history_types::query::Selection {
            profiles: args.profile,
            source: args.source,
        },
        &args.source_id,
        args.dry_run,
    )?;
    emit(&serde_json::to_value(&receipt)?, ctx.json)?;
    if !receipt.errors.is_empty() {
        bail!("some sources failed to reprocess");
    }
    Ok(())
}

pub(crate) fn usage(ctx: &Ctx, args: UsageArgs) -> Result<()> {
    let s = ctx.history();
    let selected = args.selection;
    let filter = Filter {
        profiles: selected.profile,
        source: selected.source,
        since: time_bound(&selected.since, &selected.timezone)?,
        until: time_bound(&selected.until, &selected.timezone)?,
        ..Filter::default()
    };
    emit(
        &serde_json::to_value(s.usage(
            &filter,
            &selected.timezone,
            args.weekly,
            args.by_model,
            selected.limit.unwrap_or(usize::MAX),
        )?)?,
        ctx.json,
    )?;
    Ok(())
}

pub(crate) fn sql(ctx: &Ctx, args: SqlArgs) -> Result<()> {
    let SqlArgs {
        query,
        limit,
        max_cell_bytes,
        max_output_bytes,
        full,
        no_redact,
        timeout_seconds,
        ..
    } = args;
    let s = ctx.history();
    emit(
        &s.sql(&llm_history_app::api::sql::SqlRequest {
            query,
            limit: if full { usize::MAX } else { limit },
            max_cell_bytes: if full { usize::MAX } else { max_cell_bytes },
            max_output_bytes: if full { usize::MAX } else { max_output_bytes },
            redact: !no_redact,
            timeout_seconds,
        })?,
        ctx.json,
    )?;
    Ok(())
}

pub(crate) fn db(ctx: &Ctx, command: Database) -> Result<()> {
    match command {
        Database::Compact => {
            if !ctx.db.is_file() {
                bail!("database does not exist");
            }
            emit(&ctx.history().compact()?, ctx.json)?;
        }
    }
    Ok(())
}

pub(crate) fn models(ctx: &Ctx, command: Models) -> Result<()> {
    match command {
        Models::List => emit(&json!(llm_history_models::model_artifacts()), ctx.json)?,
        Models::Fetch { download_model } => {
            if !download_model {
                bail!("model fetching requires --download-model");
            }
            let paths = llm_history_models::model_artifacts()
                .iter()
                .map(|a| llm_history_models::artifact_path(a, true))
                .collect::<Result<Vec<_>>>()?;
            emit(&json!({"models":"fetched","paths":paths}), ctx.json)?;
        }
        Models::Verify => {
            emit(&llm_history_models::verify_artifacts(&ctx.cache)?, ctx.json)?;
        }
    }
    Ok(())
}

//! Raw archive inspection and export, and database snapshots.
use super::*;

pub(crate) fn raw(ctx: &Ctx, command: Raw) -> Result<()> {
    let s = ctx.history();
    match command {
        Raw::Stats(selection) => emit(&s.raw_stats(&selection.filter(false))?, ctx.json)?,
        Raw::Sources(selection) => emit(
            &serde_json::to_value(s.raw_sources(&selection.filter(false))?)?,
            ctx.json,
        )?,
        Raw::Verify {
            selection,
            against_live,
            verbose,
        } => {
            let result = s.verify_raw(&selection.filter(against_live), against_live, verbose)?;
            emit(&result, ctx.json)?;
            if result["verified"] != true {
                bail!("some archived sources failed verification");
            }
        }
        Raw::Export {
            selection,
            output,
            stdout,
            force,
        } => {
            if selection.selector.is_none() {
                bail!("raw export requires a source ID, thread ID, or path");
            }
            let sources = s.raw_sources(&selection.filter(true))?;
            if sources.len() != 1 {
                bail!("selector is ambiguous; use --profile or a numeric source ID");
            }
            let source = &sources[0];
            if stdout {
                if ctx.json {
                    bail!("--stdout cannot be combined with --json");
                }
                s.export_raw(
                    source.id,
                    Some(source.generation),
                    &mut std::io::stdout().lock(),
                )?;
            } else {
                let output = output.context("--output or --stdout is required")?;
                if output.exists() && !force {
                    bail!("output already exists; use --force to replace it");
                }
                let parent = std::path::absolute(&output)?.parent().unwrap().to_owned();
                std::fs::create_dir_all(&parent)?;
                let mut temp = tempfile::NamedTempFile::new_in(parent)?;
                let bytes = s.export_raw(source.id, Some(source.generation), &mut temp)?;
                temp.as_file().sync_all()?;
                if force {
                    temp.persist(&output).map_err(|e| e.error)?;
                } else {
                    temp.persist_noclobber(&output).map_err(|e| e.error)?;
                }
                emit(
                    &json!({"bytes":bytes,"output":output,"source_id":source.id,"generation":source.generation,"profile":source.profile}),
                    ctx.json,
                )?;
            }
        }
    }
    Ok(())
}

pub(crate) fn archive(ctx: &Ctx, command: Archive) -> Result<()> {
    match command {
        Archive::Create { output, label } => {
            let output = if let Some(output) = output {
                output
            } else {
                let directory = ctx.db.with_file_name(format!(
                    "{}.archives",
                    ctx.db.file_name().unwrap().to_string_lossy()
                ));
                std::fs::create_dir_all(&directory)?;
                let label = label
                    .unwrap_or_default()
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>();
                directory.join(format!(
                    "{}-{}{}.duckdb",
                    ctx.db.file_stem().unwrap().to_string_lossy(),
                    chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ"),
                    if label.is_empty() {
                        String::new()
                    } else {
                        format!("-{label}")
                    }
                ))
            };
            let s = ctx.history();
            emit(&serde_json::to_value(s.snapshot(&output)?)?, ctx.json)?;
        }
        Archive::Verify { archive } => emit(
            &serde_json::to_value(llm_history_store::archives::verify_archive(&archive)?)?,
            ctx.json,
        )?,
        Archive::List { directory } => {
            let directory = directory.unwrap_or_else(|| {
                ctx.db.with_file_name(format!(
                    "{}.archives",
                    ctx.db.file_name().unwrap().to_string_lossy()
                ))
            });
            emit(
                &llm_history_store::archives::list_archives(&directory)?,
                ctx.json,
            )?;
        }
        Archive::Restore { archive, yes } => emit(
            &serde_json::to_value(llm_history_store::archives::restore_archive(
                &archive, &ctx.db, yes,
            )?)?,
            ctx.json,
        )?,
    }
    Ok(())
}

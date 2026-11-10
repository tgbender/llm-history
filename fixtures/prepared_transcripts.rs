//! Populate storage tests using small synthetic transcripts and the typed publisher.
//! Application workflow behavior is covered through History's contract tests.
use super::Store;
use anyhow::{Result, bail};
use llm_history_ingest::{
    PARSER_VERSION, ParserState,
    prepare::{self, CaptureWork},
    source::discover,
};
use llm_history_types::ingestion::{CaptureKind, ReprocessReceipt, UpdateReceipt};
use std::path::Path;

pub fn capture(store: &Store, names: &[String], repair: bool) -> Result<UpdateReceipt> {
    let mut result = UpdateReceipt::default();
    for profile in store.profiles(names, true)? {
        if !repair {
            let ids = store
                .source_checkpoints(&profile)?
                .into_iter()
                .filter(|s| s.decoder_revision != PARSER_VERSION)
                .map(|s| s.id)
                .collect::<Vec<_>>();
            if !ids.is_empty() {
                result.errors.extend(
                    replay(
                        store,
                        std::slice::from_ref(&profile.name),
                        None,
                        &ids,
                        false,
                    )?
                    .errors,
                );
            }
        }
        let sources = discover(&profile)?;
        let keys = sources.iter().map(|source| source.key.clone()).collect();
        let checkpoints = store.source_checkpoints(&profile)?;
        let index = prepare::CheckpointIndex::new(&checkpoints);
        for source in sources {
            let published = (|| -> Result<_> {
                match index.plan(&source, repair)? {
                    CaptureWork::Unchanged(previous) => {
                        if previous.missing || previous.uri != source.path.to_string_lossy() {
                            store.refresh_source_locations(
                                &profile,
                                &[(previous, source.path.clone())],
                            )?;
                        }
                        Ok(None)
                    }
                    CaptureWork::Read(previous) => {
                        let mut rows = Vec::new();
                        let reader = prepare::open_capture(&source, previous, repair)?;
                        let usage = match reader.usage_source() {
                            Some(id) => store.usage_keys(&[id])?.remove(&id).unwrap_or_default(),
                            None => Default::default(),
                        };
                        let capture = reader.read(usage, |r| {
                            rows.push(r);
                            Ok(())
                        })?;
                        let outcomes = store
                            .publish_captures(&profile, [(&capture, rows.into_iter().map(Ok))])?;
                        Ok(outcomes.into_iter().next())
                    }
                }
            })();
            match published {
                Ok(None | Some((CaptureKind::Append, 0))) => result.unchanged += 1,
                Ok(Some((kind, count))) => {
                    result.imported += 1;
                    result.appended += usize::from(kind == CaptureKind::Append);
                    result.records += count;
                }
                Err(e) => result
                    .errors
                    .push(format!("{}: {e:#}", source.path.display())),
            }
        }
        result.missing += store.reconcile_inventory(&profile, &keys)?;
        match prepare::catalog(&profile, store.catalog_revision(&profile)?) {
            Ok(catalog) => {
                let c = store.publish_catalog(&profile, catalog)?;
                result.catalogs.snapshots += c.snapshots;
                result.catalogs.projects += c.projects;
                result.catalogs.assignments += c.assignments;
            }
            Err(_) => result.catalogs.errors.push(format!(
                "{}: catalog capture failed; previous metadata retained",
                profile.name
            )),
        }
    }
    result.errors.extend(result.catalogs.errors.iter().cloned());
    store.reconcile_conversations(repair)?;
    Ok(result)
}

pub fn replay(
    store: &Store,
    names: &[String],
    harness: Option<&str>,
    ids: &[i64],
    dry_run: bool,
) -> Result<ReprocessReceipt> {
    let mut result = ReprocessReceipt {
        dry_run,
        ..Default::default()
    };
    let mut sources = Vec::new();
    for profile in store.profiles(names, false)? {
        if harness.is_none_or(|s| s == "all" || s == profile.harness.key()) {
            sources.extend(
                store
                    .replay_sources(&profile, ids)?
                    .into_iter()
                    .filter(|s| ids.is_empty() || ids.contains(&s.checkpoint.id)),
            );
        }
    }
    if ids
        .iter()
        .any(|id| !sources.iter().any(|s| s.checkpoint.id == *id))
    {
        bail!("source ID not found in selected profiles/harness");
    }
    let mut inputs = Vec::new();
    for source in &sources {
        let mut parser =
            ParserState::new(Path::new(&source.checkpoint.key), source.profile.harness);
        let mut rows = Vec::new();
        if store
            .retained_records(&[source], |_, r| {
                rows.push(prepare::project_retained(
                    &mut parser,
                    source.profile.harness,
                    r,
                ));
                Ok(())
            })
            .is_err()
        {
            result.errors.push(format!(
                "source {}: archive reprocessing failed; previous projection retained",
                source.checkpoint.id
            ));
            continue;
        }
        let projection = parser.projection_checkpoint()?;
        result.sources += 1;
        result.records += rows.len();
        for fact in rows.iter().flat_map(|r| &r.facts) {
            *result.facts.entry(fact.kind.clone()).or_insert(0) += 1;
        }
        inputs.push((source, projection, rows));
    }
    if !dry_run && !inputs.is_empty() {
        store.publish_projections(
            inputs
                .iter_mut()
                .map(|(source, projection, rows)| {
                    (
                        *source,
                        &*projection,
                        std::mem::take(rows).into_iter().map(Ok),
                    )
                })
                .collect(),
        )?;
    }
    if !dry_run {
        let profiles: std::collections::BTreeMap<_, _> = sources
            .iter()
            .map(|s| (&s.profile.id, &s.profile))
            .collect();
        store.reconcile_pending_projects(profiles.into_values())?;
        store.reconcile_conversations(false)?;
    }
    Ok(result)
}

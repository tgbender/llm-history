//! Application coordination for native capture and publication.
use crate::History;
use anyhow::{Error, Result, bail};
use llm_history_ingest::{
    PARSER_VERSION, ParserState,
    prepare::{self, CaptureWork},
    source::discover,
};
use llm_history_store::Store;
use llm_history_types::{
    Profile,
    ingestion::{
        CaptureKind, CatalogReceipt, PreparedCapture, ProjectionCheckpoint, ReplaySource,
        ReprocessReceipt, UpdateReceipt,
    },
};
use std::{collections::BTreeMap, path::Path};

mod spool;

const CHUNK_BYTES: u64 = 256 * 1024 * 1024;
const CHUNK_SOURCES: usize = 64;

fn publish_captures(
    history: &History,
    profile: &Profile,
    batch: &[(PreparedCapture, spool::Prepared)],
    receipt: &mut UpdateReceipt,
) -> Result<()> {
    let store = history.open(false)?;
    let publish = |batch: &[(PreparedCapture, spool::Prepared)]| {
        let inputs = batch
            .iter()
            .map(|(work, spool)| Ok((work, spool.records()?)))
            .collect::<Result<Vec<_>>>()?;
        store.publish_captures(profile, inputs)
    };
    let outcomes = match publish(batch) {
        Ok(outcomes) => outcomes.into_iter().map(Ok).collect::<Vec<_>>(),
        Err(error) if batch.len() == 1 => vec![Err(error)],
        Err(_) => batch
            .iter()
            .map(|input| publish(std::slice::from_ref(input)).map(|mut rows| rows.remove(0)))
            .collect(),
    };
    for ((work, _), outcome) in batch.iter().zip(outcomes) {
        match outcome {
            Ok((CaptureKind::Append, 0)) => receipt.unchanged += 1,
            Ok((kind, count)) => {
                receipt.appended += usize::from(kind == CaptureKind::Append);
                receipt.imported += 1;
                receipt.records += count;
            }
            Err(error) => receipt
                .errors
                .push(format!("{}: {error:#}", work.path.display())),
        }
    }
    Ok(())
}

pub(crate) fn update(history: &History, names: &[String], repair: bool) -> Result<UpdateReceipt> {
    let mut receipt = UpdateReceipt::default();
    let profiles = history.open(true)?.profiles(names, true)?;
    for profile in profiles {
        let mut checkpoints = history.open(true)?.source_checkpoints(&profile)?;
        if !repair {
            let stale = checkpoints
                .iter()
                .filter(|s| s.decoder_revision != PARSER_VERSION)
                .map(|s| s.id)
                .collect::<Vec<_>>();
            if !stale.is_empty() {
                receipt.errors.extend(
                    reprocess(
                        history,
                        std::slice::from_ref(&profile.name),
                        None,
                        &stale,
                        false,
                    )?
                    .errors,
                );
                checkpoints = history.open(true)?.source_checkpoints(&profile)?;
            }
        }
        let sources = discover(&profile)?;
        let keys = sources.iter().map(|source| source.key.clone()).collect();
        let index = prepare::CheckpointIndex::new(&checkpoints);
        let mut batch = Vec::new();
        let mut bytes = 0;
        for group in sources.chunks(CHUNK_SOURCES) {
            let mut opened = Vec::new();
            let mut locations = Vec::new();
            for source in group {
                let outcome = (|| {
                    match index.plan(source, repair)? {
                        CaptureWork::Unchanged(previous) => {
                            if previous.missing || previous.uri != source.path.to_string_lossy() {
                                locations.push((previous, source.path.clone()));
                            } else {
                                receipt.unchanged += 1;
                            }
                        }
                        CaptureWork::Read(previous) => opened.push((
                            &source.path,
                            prepare::open_capture(source, previous, repair)?,
                        )),
                    }
                    Ok::<_, anyhow::Error>(())
                })();
                if let Err(error) = outcome {
                    receipt
                        .errors
                        .push(format!("{}: {error:#}", source.path.display()));
                }
            }
            if !locations.is_empty() {
                let store = history.open(false)?;
                match store.refresh_source_locations(&profile, &locations) {
                    Ok(()) => receipt.unchanged += locations.len(),
                    Err(_) => {
                        for change in &locations {
                            match store
                                .refresh_source_locations(&profile, std::slice::from_ref(change))
                            {
                                Ok(()) => receipt.unchanged += 1,
                                Err(error) => receipt
                                    .errors
                                    .push(format!("{}: {error:#}", change.1.display())),
                            }
                        }
                    }
                }
            }
            let usage_sources = opened
                .iter()
                .filter_map(|(_, reader)| reader.usage_source())
                .collect::<Vec<_>>();
            let mut usage = if usage_sources.is_empty() {
                Ok(Default::default())
            } else {
                history.open(true)?.usage_keys(&usage_sources)
            };
            for (path, reader) in opened {
                let result = (|| {
                    let keys = match reader.usage_source() {
                        Some(id) => usage
                            .as_mut()
                            .map_err(|e| anyhow::anyhow!("{e:#}"))?
                            .remove(&id)
                            .unwrap_or_default(),
                        None => Default::default(),
                    };
                    let mut spool = spool::Spool::new()?;
                    let work = reader.read(keys, |r| spool.push(r))?;
                    Ok::<_, anyhow::Error>((work, spool.finish()?))
                })();
                match result {
                    Ok((work, prepared)) => {
                        if !batch.is_empty()
                            && (bytes + prepared.size() > CHUNK_BYTES
                                || batch.len() >= CHUNK_SOURCES)
                        {
                            publish_captures(history, &profile, &batch, &mut receipt)?;
                            batch.clear();
                            bytes = 0;
                        }
                        bytes += prepared.size();
                        batch.push((work, prepared));
                        if bytes >= CHUNK_BYTES {
                            publish_captures(history, &profile, &batch, &mut receipt)?;
                            batch.clear();
                            bytes = 0;
                        }
                    }
                    Err(error) => receipt
                        .errors
                        .push(format!("{}: {error:#}", path.display())),
                }
            }
        }
        if !batch.is_empty() {
            publish_captures(history, &profile, &batch, &mut receipt)?;
        }
        receipt.missing += history.open(false)?.reconcile_inventory(&profile, &keys)?;
    }
    receipt.catalogs = refresh_catalogs(history, names)?;
    receipt
        .errors
        .extend(receipt.catalogs.errors.iter().cloned());
    history.open(false)?.reconcile_conversations(repair)?;
    Ok(receipt)
}

pub(crate) fn refresh_catalogs(history: &History, names: &[String]) -> Result<CatalogReceipt> {
    let mut receipt = CatalogReceipt::default();
    let profiles = history.open(true)?.profiles(names, true)?;
    for profile in profiles {
        let revision = history.open(true)?.catalog_revision(&profile)?;
        match prepare::catalog(&profile, revision) {
            Ok(input) => {
                let result = history.open(false)?.publish_catalog(&profile, input)?;
                receipt.snapshots += result.snapshots;
                receipt.projects += result.projects;
                receipt.assignments += result.assignments;
            }
            Err(_) => receipt.errors.push(format!(
                "{}: catalog capture failed; previous metadata retained",
                profile.name
            )),
        }
    }
    Ok(receipt)
}

struct Replay<'a> {
    source: &'a ReplaySource,
    projection: ProjectionCheckpoint,
    prepared: Option<spool::Prepared>,
    facts: BTreeMap<String, usize>,
}

fn prepare_replays<'a>(
    history: &History,
    sources: &'a [ReplaySource],
    dry_run: bool,
) -> Result<Vec<Replay<'a>>> {
    let mut builders: Vec<_> = sources
        .iter()
        .map(|source| {
            Ok((
                ParserState::new(Path::new(&source.checkpoint.key), source.profile.harness),
                (!dry_run).then(spool::Spool::new).transpose()?,
                BTreeMap::new(),
            ))
        })
        .collect::<Result<_>>()?;
    let selected: Vec<_> = sources.iter().collect();
    history
        .open(true)?
        .retained_records(&selected, |index, raw| {
            let (parser, spool, facts) = &mut builders[index];
            let record = prepare::project_retained(parser, sources[index].profile.harness, raw);
            for fact in &record.facts {
                *facts.entry(fact.kind.clone()).or_insert(0) += 1;
            }
            if let Some(spool) = spool {
                spool.push(record)?;
            }
            Ok(())
        })?;
    sources
        .iter()
        .zip(builders)
        .map(|(source, (parser, spool, facts))| {
            Ok(Replay {
                source,
                projection: parser.projection_checkpoint()?,
                prepared: spool.map(spool::Spool::finish).transpose()?,
                facts,
            })
        })
        .collect()
}

fn publish_replays(
    store: Option<&Store>,
    batch: &[Replay<'_>],
    receipt: &mut ReprocessReceipt,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    if let Some(store) = store {
        let inputs = batch
            .iter()
            .map(|r| {
                Ok((
                    r.source,
                    &r.projection,
                    r.prepared.as_ref().unwrap().records()?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        store.publish_projections(inputs)?;
    }
    for replay in batch {
        receipt.sources += 1;
        receipt.records += replay.source.checkpoint.ordinal as usize;
        for (kind, count) in &replay.facts {
            *receipt.facts.entry(kind.clone()).or_insert(0) += count;
        }
    }
    Ok(())
}

pub(crate) fn reprocess(
    history: &History,
    names: &[String],
    harness: Option<&str>,
    ids: &[i64],
    dry_run: bool,
) -> Result<ReprocessReceipt> {
    const CHUNK_RECORDS: u64 = 250_000;
    const CHUNK_SOURCES: usize = 8;
    let sources = {
        let store = history.open(true)?;
        let mut sources = Vec::new();
        for profile in store.profiles(names, false)? {
            if harness.is_none_or(|s| s == "all" || s == profile.harness.key()) {
                sources.extend(store.replay_sources(&profile, ids)?);
            }
        }
        sources
    };
    if ids
        .iter()
        .any(|id| !sources.iter().any(|s| s.checkpoint.id == *id))
    {
        bail!("source ID not found in selected profiles/harness");
    }
    let mut receipt = ReprocessReceipt {
        dry_run,
        ..Default::default()
    };
    let error = |id, stage, error: Error| {
        let cause = llm_history_core::bounded(&format!("{error:#}"), 800, true);
        format!("source {id}: {stage} failed; previous projection retained: {cause}")
    };
    let flush = |chunk: &mut Vec<Replay<'_>>, receipt: &mut ReprocessReceipt| -> Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        let store = (!dry_run).then(|| history.open(false)).transpose()?;
        if let Err(batch_error) = publish_replays(store.as_ref(), chunk, receipt) {
            if chunk.len() == 1 {
                receipt.errors.push(error(
                    chunk[0].source.checkpoint.id,
                    "replay publication",
                    batch_error,
                ));
            } else {
                for replay in chunk.iter() {
                    if let Err(cause) =
                        publish_replays(store.as_ref(), std::slice::from_ref(replay), receipt)
                    {
                        receipt.errors.push(error(
                            replay.source.checkpoint.id,
                            "replay publication",
                            cause,
                        ));
                    }
                }
            }
        }
        chunk.clear();
        Ok(())
    };
    let mut remaining = sources.as_slice();
    while !remaining.is_empty() {
        let mut count = 0;
        let (mut records, mut bytes) = (0u64, 0u64);
        for source in remaining.iter().take(CHUNK_SOURCES) {
            let s = &source.checkpoint;
            if count > 0
                && (records.saturating_add(s.ordinal) > CHUNK_RECORDS
                    || bytes.saturating_add(s.committed) > CHUNK_BYTES)
            {
                break;
            }
            records = records.saturating_add(s.ordinal);
            bytes = bytes.saturating_add(s.committed);
            count += 1;
        }
        let (group, rest) = remaining.split_at(count);
        remaining = rest;
        let prepared = match prepare_replays(history, group, dry_run) {
            Ok(rows) => rows,
            Err(_) if group.len() > 1 => group
                .iter()
                .flat_map(|source| {
                    match prepare_replays(history, std::slice::from_ref(source), dry_run) {
                        Ok(rows) => rows,
                        Err(cause) => {
                            receipt.errors.push(error(
                                source.checkpoint.id,
                                "replay preparation",
                                cause,
                            ));
                            Vec::new()
                        }
                    }
                })
                .collect(),
            Err(cause) => {
                receipt
                    .errors
                    .push(error(group[0].checkpoint.id, "replay preparation", cause));
                continue;
            }
        };
        let mut chunk = Vec::new();
        let mut bytes = 0u64;
        for replay in prepared {
            let size = replay.prepared.as_ref().map_or(0, spool::Prepared::size);
            if !chunk.is_empty() && bytes.saturating_add(size) > CHUNK_BYTES {
                flush(&mut chunk, &mut receipt)?;
                bytes = 0;
            }
            bytes += size;
            chunk.push(replay);
        }
        flush(&mut chunk, &mut receipt)?;
    }
    if !dry_run {
        let profiles: BTreeMap<_, _> = sources
            .iter()
            .map(|s| (&s.profile.id, &s.profile))
            .collect();
        let store = history.open(false)?;
        store.reconcile_pending_projects(profiles.into_values())?;
        store.reconcile_conversations(false)?;
    }
    Ok(receipt)
}

//! Filesystem capture and native preparation. No destination database access.
use crate::{ParserState, content, source::Transcript};
use anyhow::{Context, Result};
use llm_history_core::{
    Capture, RawRecord, hash_range, logical_path, mtime_ns, read_windows, window_hashes,
};
use llm_history_types::{
    Harness, Profile,
    ingestion::{
        CaptureEncoding, CaptureKind, PreparedCapture, PreparedRaw, PreparedRecord, RetainedRecord,
        SourceCheckpoint,
    },
};
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
};

const MAX_RECORD: usize = 64 * 1024 * 1024;

pub fn catalog(
    profile: &Profile,
    previous: Option<String>,
) -> Result<llm_history_types::ingestion::PreparedCatalog> {
    let fingerprint = crate::catalog::catalog_fingerprint(profile)?;
    let changed = previous.as_deref() != Some(fingerprint.as_str());
    let loaded = if changed {
        crate::catalog::load_catalog(profile)?
    } else {
        None
    };
    let threads = if changed && profile.harness == Harness::Codex {
        Some(crate::codex::source::read_threads(&profile.root)?)
    } else {
        None
    };
    Ok(llm_history_types::ingestion::PreparedCatalog {
        previous,
        fingerprint,
        loaded,
        threads,
    })
}

pub enum CaptureWork {
    Unchanged(SourceCheckpoint),
    Read(Option<SourceCheckpoint>),
}

pub struct CheckpointIndex<'a> {
    exact: HashMap<&'a str, &'a SourceCheckpoint>,
    basenames: HashMap<&'a str, Vec<&'a SourceCheckpoint>>,
}
impl<'a> CheckpointIndex<'a> {
    pub fn new(checkpoints: &'a [SourceCheckpoint]) -> Self {
        let mut exact = HashMap::with_capacity(checkpoints.len());
        let mut basenames = HashMap::<_, Vec<_>>::new();
        for row in checkpoints {
            exact.insert(row.key.as_str(), row);
            let basename = row.key.rsplit(['/', '\\']).next().unwrap_or("");
            basenames.entry(basename).or_default().push(row);
        }
        Self { exact, basenames }
    }

    pub fn plan(&self, source: &Transcript, repair: bool) -> Result<CaptureWork> {
        let path = &source.path;
        let exact = self.exact.get(source.key.as_str()).copied();
        if let Some(row) = exact.filter(|_| !repair) {
            let observed = source.observe()?;
            if (source.harness != Harness::Cursor || row.uri == path.to_string_lossy())
                && observed.matches(row.size, row.mtime_ns, &row.prefix_hash)
            {
                return Ok(CaptureWork::Unchanged(row.clone()));
            }
        }
        if source.harness == Harness::Cursor {
            return Ok(CaptureWork::Read(exact.cloned()));
        }
        let previous = exact
            .or_else(|| {
                let logical = logical_path(path);
                let basename = logical.file_name()?.to_str()?;
                let mut matches = self.basenames.get(basename)?.iter().copied().filter(|row| {
                    let uri = row.uri.strip_prefix("file://").unwrap_or(&row.uri);
                    if Path::new(uri).exists() {
                        return false;
                    }
                    match (row.head, row.boundary) {
                        (Some(head), Some(boundary)) => read_windows(path, row.committed)
                            .is_ok_and(|(h, t)| window_hashes(&h, &t) == (head, boundary)),
                        _ => false,
                    }
                });
                let first = matches.next();
                if matches.next().is_none() {
                    first
                } else {
                    None
                }
            })
            .cloned();
        Ok(CaptureWork::Read(previous))
    }
}

/// Freeze and validate the source before looking up any historical usage keys.
pub struct OpenCapture<'a> {
    source: &'a Transcript,
    previous: Option<SourceCheckpoint>,
    input: Option<Capture>,
    parser: ParserState,
    kind: CaptureKind,
}

pub fn open_capture(
    source: &Transcript,
    previous: Option<SourceCheckpoint>,
    repair: bool,
) -> Result<OpenCapture<'_>> {
    let path = &source.path;
    let harness = source.harness;
    let logical = logical_path(path);
    let mut parser = ParserState::new(
        if harness == Harness::Cursor {
            Path::new(&source.key)
        } else {
            &logical
        },
        harness,
    );
    let mut kind = if previous.is_none() {
        CaptureKind::New
    } else if repair {
        CaptureKind::Repair
    } else {
        CaptureKind::Rewrite
    };
    if source.sqlite {
        if previous.is_some() && !repair {
            kind = CaptureKind::Snapshot;
        }
        return Ok(OpenCapture {
            source,
            previous,
            input: None,
            parser,
            kind,
        });
    }
    let mut resumed = None;
    if let Some(row) = previous.as_ref().filter(|r| {
        !repair
            && !r
                .prefix_hash
                .starts_with(crate::cursor::source::SNAPSHOT_TOKEN_PREFIX)
    }) {
        resumed = match (row.head, row.boundary) {
            (Some(head), Some(boundary)) => Capture::resume(
                path,
                row.committed,
                row.ordinal,
                MAX_RECORD,
                (head, boundary),
            )?,
            _ if hash_range(path, 0, row.committed).is_ok_and(|h| h == row.prefix_hash) => {
                Some(Capture::open(
                    path,
                    row.committed,
                    row.ordinal,
                    MAX_RECORD,
                    read_windows(path, row.committed)?,
                )?)
            }
            _ => None,
        };
        if resumed.is_some() {
            parser = serde_json::from_str(&row.continuation)
                .context("invalid retained decoder checkpoint")?;
            kind = CaptureKind::Append;
        }
    }
    let input = match resumed {
        Some(input) => input,
        None => Capture::open(path, 0, 0, MAX_RECORD, Default::default())?,
    };
    Ok(OpenCapture {
        source,
        previous,
        input: Some(input),
        parser,
        kind,
    })
}

impl OpenCapture<'_> {
    pub fn usage_source(&self) -> Option<i64> {
        (self.kind == CaptureKind::Append && self.source.harness == Harness::Claude)
            .then(|| self.previous.as_ref().map(|row| row.id))
            .flatten()
    }

    pub fn read(
        self,
        usage: BTreeSet<String>,
        mut emit: impl FnMut(PreparedRecord) -> Result<()>,
    ) -> Result<PreparedCapture> {
        let Self {
            source,
            previous,
            input,
            mut parser,
            kind,
        } = self;
        parser.seen_usage = usage;
        let (path, harness) = (&source.path, source.harness);
        let Some(mut input) = input else {
            return crate::cursor::source::capture(source, previous, parser, kind, emit);
        };
        let start = input.committed;
        let start_ordinal = input.ordinal;
        while let Some(record) = input.next_record()? {
            emit(project_raw(&mut parser, harness, record))?;
        }
        input.check_source()?;
        Ok(PreparedCapture {
            expected: previous,
            key: source.key.clone(),
            encoding: if llm_history_core::is_compressed(path) {
                CaptureEncoding::ZstdJsonl
            } else {
                CaptureEncoding::Jsonl
            },
            source_token: String::new(),
            path: path.clone(),
            kind,
            physical_cutoff: input.initial.len(),
            mtime_ns: mtime_ns(&input.initial),
            start,
            start_ordinal,
            end: input.committed,
            end_ordinal: input.ordinal,
            complete: input.complete,
            windows: input.window_hashes(),
            projection: parser.projection_checkpoint()?,
        })
    }
}

pub fn project_raw(
    parser: &mut ParserState,
    harness: Harness,
    record: RawRecord,
) -> PreparedRecord {
    let json_valid = record.value.is_some();
    let (session, facts) = project(parser, harness, record.ordinal, record.value);
    let leaves = content::leaves(harness, &record.bytes);
    PreparedRecord {
        ordinal: record.ordinal,
        session,
        facts,
        document_hash: String::new(),
        retained_raw_id: None,
        raw: Some(PreparedRaw {
            start: record.start,
            end: record.end,
            bytes: record.bytes,
            leaves,
            json_valid,
        }),
    }
}

pub fn project_retained(
    parser: &mut ParserState,
    harness: Harness,
    record: RetainedRecord,
) -> PreparedRecord {
    let value = serde_json::from_slice(&record.bytes)
        .ok()
        .filter(serde_json::Value::is_object);
    let (session, facts) = project(parser, harness, record.ordinal, value);
    PreparedRecord {
        ordinal: record.ordinal,
        session,
        facts,
        document_hash: record.hash,
        retained_raw_id: record.raw_id,
        raw: None,
    }
}

fn project(
    parser: &mut ParserState,
    harness: Harness,
    ordinal: u64,
    value: Option<serde_json::Value>,
) -> (
    Option<llm_history_types::ingestion::SessionUpdate>,
    Vec<llm_history_types::Fact>,
) {
    if let Some(value) = value {
        let facts = parser.parse_record(harness, &value, ordinal);
        (Some(parser.session_update()), facts)
    } else {
        (None, Vec::new())
    }
}

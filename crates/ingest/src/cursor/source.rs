//! Source inventory and consistent acquisition from the local Cursor CLI store.
use super::wire;
use crate::{ParserState, prepare::project_raw, source::Transcript};
use anyhow::{Context, Result, ensure};
use llm_history_core::{RawRecord, mtime_ns};
use llm_history_types::{
    Harness,
    ingestion::{CaptureEncoding, CaptureKind, PreparedCapture, PreparedRecord, SourceCheckpoint},
};
use rusqlite::{Connection, OpenFlags, Statement};
use serde::Serialize;
use serde_json::{Value, value::RawValue};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, DirEntry, File},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;

const ROW_LIMIT: usize = 100_000;
const BLOB_LIMIT: usize = 16 * 1024 * 1024;
const BYTE_LIMIT: usize = 128 * 1024 * 1024;
const METADATA_LIMIT: usize = 1024 * 1024;
pub(crate) const SNAPSHOT_TOKEN_PREFIX: &str = "sqlite:";

fn entries(path: &Path) -> Result<Vec<DirEntry>> {
    match fs::read_dir(path) {
        Ok(entries) => Ok(entries.collect::<std::io::Result<_>>()?),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn id(value: &OsStr) -> Option<String> {
    Uuid::parse_str(value.to_str()?).ok().map(|v| v.to_string())
}

fn transcript_id(path: &Path) -> Option<String> {
    (path.extension()? == "jsonl").then_some(())?;
    id(path.file_stem()?)
}

fn source(path: PathBuf, session: &str, parent: Option<&str>, sqlite: bool) -> Transcript {
    let key = match parent {
        Some(p) => format!("cursor/{p}/subagents/{session}"),
        None => format!("cursor/{session}"),
    };
    Transcript {
        path,
        key,
        harness: Harness::Cursor,
        sqlite,
    }
}

pub(crate) fn discover(root: &Path) -> Result<Vec<Transcript>> {
    let mut transcripts = BTreeMap::<String, Transcript>::new();
    for workspace in entries(&root.join("projects"))? {
        if !workspace.file_type()?.is_dir() {
            continue;
        }
        let mut local = BTreeMap::new();
        for entry in entries(&workspace.path().join("agent-transcripts"))? {
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_file()
                && let Some(session) = transcript_id(&path)
            {
                local
                    .entry(session.clone())
                    .or_insert_with(|| source(path, &session, None, false));
            } else if ty.is_dir()
                && let Some(session) = id(&entry.file_name())
            {
                for child in entries(&path)? {
                    if child.file_type()?.is_file()
                        && transcript_id(&child.path()).as_deref() == Some(&session)
                    {
                        local.insert(session.clone(), source(child.path(), &session, None, false));
                    }
                }
                for child in entries(&path.join("subagents"))? {
                    if child.file_type()?.is_file()
                        && let Some(child_id) = transcript_id(&child.path())
                    {
                        ensure!(
                            local
                                .insert(
                                    child_id.clone(),
                                    source(child.path(), &child_id, Some(&session), false)
                                )
                                .is_none(),
                            "ambiguous Cursor transcript for {child_id}"
                        );
                    }
                }
            }
        }
        for (session, transcript) in local {
            ensure!(
                transcripts.insert(session.clone(), transcript).is_none(),
                "ambiguous Cursor transcript across workspaces for {session}"
            );
        }
    }
    let mut stores = BTreeMap::new();
    for workspace in entries(&root.join("chats"))? {
        if !workspace.file_type()?.is_dir() {
            continue;
        }
        for dir in entries(&workspace.path())? {
            if !dir.file_type()?.is_dir() {
                continue;
            }
            let path = dir.path().join("store.db");
            let Some(session) = id(&dir.file_name()) else {
                continue;
            };
            let present = match fs::symlink_metadata(&path) {
                Ok(meta) => meta.is_file(),
                Err(error) if error.kind() == ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            if present {
                ensure!(
                    stores.insert(session.clone(), path).is_none(),
                    "ambiguous Cursor chat database across workspaces for {session}"
                );
            }
        }
    }
    for (session, path) in stores {
        let mut selected = source(path, &session, None, true);
        if let Some(transcript) = transcripts.get(&session) {
            selected.key.clone_from(&transcript.key);
        }
        transcripts.insert(session, selected);
    }
    Ok(transcripts.into_values().collect())
}

pub(crate) fn snapshot_token(path: &Path) -> Result<String> {
    // A WAL commit can leave the main database's size and mtime unchanged.
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let stat = |path: &Path| match path.metadata() {
        Ok(meta) => Ok(Some((meta.len(), mtime_ns(&meta)))),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    };
    Ok(format!(
        "{SNAPSHOT_TOKEN_PREFIX}{}",
        serde_json::to_string(&[
            stat(Path::new(&wal))?.filter(|(size, _)| *size > 0),
            stat(&path.with_file_name("meta.json"))?,
        ])?
    ))
}

fn sidecar(path: &Path) -> Result<Option<Box<RawValue>>> {
    let file = match File::open(path.with_file_name("meta.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    file.take(METADATA_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= METADATA_LIMIT,
        "Cursor metadata exceeds read limit"
    );
    Ok(Some(
        serde_json::from_slice(&bytes).context("invalid Cursor meta.json")?,
    ))
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum RetainedRow<'a> {
    #[serde(rename = "cursor_store_meta")]
    Meta {
        meta: &'a RawValue,
        sidecar: Option<&'a RawValue>,
        root_blob_hex: String,
    },
    #[serde(rename = "cursor_store_tree")]
    Tree { blob_id: String, blob_hex: String },
    #[serde(rename = "cursor_store_message")]
    Message {
        blob_id: String,
        message: &'a RawValue,
        #[serde(skip_serializing_if = "Option::is_none")]
        native_blob_hex: Option<String>,
    },
}

struct Capture<'a, F> {
    lookup: Statement<'a>,
    deadline: Instant,
    reads: usize,
    bytes: usize,
    end: u64,
    ordinal: u64,
    parser: ParserState,
    emit: F,
}

impl<F: FnMut(PreparedRecord) -> Result<()>> Capture<'_, F> {
    fn blob(&mut self, id: &str) -> Result<Vec<u8>> {
        self.reads += 1;
        ensure!(
            self.reads <= ROW_LIMIT && Instant::now() < self.deadline,
            "Cursor snapshot read budget exhausted"
        );
        let data = self
            .lookup
            .query_row([id], |row| row.get::<_, Vec<u8>>(0))
            .context("missing or unreadable Cursor blob in the selected conversation")?;
        self.bytes += data.len();
        ensure!(
            self.bytes <= BYTE_LIMIT,
            "Cursor snapshot exceeds read byte limit"
        );
        Ok(data)
    }

    fn record(&mut self, row: RetainedRow<'_>) -> Result<()> {
        let mut bytes = serde_json::to_vec(&row)?;
        let value = serde_json::from_slice(&bytes)?;
        bytes.push(b'\n');
        let start = self.end;
        self.end += bytes.len() as u64;
        self.ordinal += 1;
        (self.emit)(project_raw(
            &mut self.parser,
            Harness::Cursor,
            RawRecord {
                ordinal: self.ordinal,
                start,
                end: self.end,
                bytes,
                value: Some(value),
            },
        ))
    }

    fn messages(&mut self, tree: &[u8], depth: usize) -> Result<()> {
        ensure!(depth < 32, "Cursor message tree exceeds depth limit");
        let refs = wire::references(tree, ROW_LIMIT)?;
        ensure!(
            !refs.is_empty(),
            "Cursor store has no supported message projection"
        );
        for (subtree, reference) in refs {
            let blob_id = hex::encode(reference);
            let data = self.blob(&blob_id)?;
            if subtree {
                self.record(RetainedRow::Tree {
                    blob_id,
                    blob_hex: hex::encode(&data),
                })?;
                self.messages(&data, depth + 1)?;
            } else {
                let message = wire::message(&data)?;
                self.record(RetainedRow::Message {
                    blob_id,
                    message,
                    native_blob_hex: (message.get().as_bytes() != data).then(|| hex::encode(&data)),
                })?;
            }
        }
        Ok(())
    }
}

pub(crate) fn capture(
    source: &Transcript,
    previous: Option<SourceCheckpoint>,
    parser: ParserState,
    kind: CaptureKind,
    emit: impl FnMut(PreparedRecord) -> Result<()>,
) -> Result<PreparedCapture> {
    let path = &source.path;
    let observed = source.observe()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let con = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    con.busy_timeout(Duration::from_millis(100))?;
    con.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        BLOB_LIMIT.try_into()?,
    )?;
    con.progress_handler(10_000, Some(move || Instant::now() >= deadline))?;
    con.execute_batch("PRAGMA query_only=true; BEGIN")?;
    let encoded = con.query_row("SELECT value FROM meta WHERE key='0'", [], |row| {
        row.get::<_, String>(0)
    })?;
    ensure!(
        encoded.len() <= 2 * METADATA_LIMIT,
        "Cursor metadata exceeds read limit"
    );
    let raw_meta = hex::decode(encoded.trim()).context("invalid Cursor store metadata encoding")?;
    let meta: Value = serde_json::from_slice(&raw_meta)?;
    let root_id = meta["latestRootBlobId"]
        .as_str()
        .context("Cursor store has no selected root")?;
    ensure!(
        root_id.len() == 64 && root_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid Cursor root ID"
    );
    if let Some(native) = meta["agentId"].as_str() {
        ensure!(
            Uuid::parse_str(native)
                .ok()
                .is_some_and(|u| u.to_string() == parser.canonical.runtime_id),
            "Cursor store identity does not match its session directory"
        );
    }
    let sidecar = sidecar(path)?;
    let mut capture = Capture {
        lookup: con.prepare("SELECT data FROM blobs WHERE id=?")?,
        deadline,
        reads: 0,
        bytes: 0,
        end: 0,
        ordinal: 0,
        parser,
        emit,
    };
    let root = capture.blob(root_id)?;
    capture.record(RetainedRow::Meta {
        meta: serde_json::from_slice(&raw_meta)?,
        sidecar: sidecar.as_deref(),
        root_blob_hex: hex::encode(&root),
    })?;
    capture.messages(&root, 0)?;
    ensure!(
        source.observe()? == observed,
        "Cursor source changed during capture"
    );
    Ok(PreparedCapture {
        expected: previous,
        key: source.key.clone(),
        path: path.clone(),
        kind,
        encoding: CaptureEncoding::SqliteRows,
        source_token: observed.token,
        physical_cutoff: observed.size,
        mtime_ns: observed.mtime_ns,
        start: 0,
        start_ordinal: 0,
        end: capture.end,
        end_ordinal: capture.ordinal,
        complete: true,
        windows: (0, 0),
        projection: capture.parser.projection_checkpoint()?,
    })
}

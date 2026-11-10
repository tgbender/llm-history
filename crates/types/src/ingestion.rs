//! Import, catalog and replay outcome values.
use crate::{Fact, Profile, canonical};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A reusable UTF-8 body and its exact token location in a captured JSON record.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContentLeaf {
    pub token: std::ops::Range<usize>,
    pub native_path: String,
    pub body: String,
}

/// Adapter's final or intermediate session metadata, independent of its decoder state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionUpdate {
    pub canonical: canonical::Session,
    pub timezone: String,
    pub session: String,
    pub cwd: String,
    pub model: String,
    pub title: String,
    pub parent: String,
    pub relation: String,
    pub subagent: bool,
    pub last_prompt: String,
    pub first_user_message: String,
}

/// State the application observed before preparing a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCheckpoint {
    pub id: i64,
    pub key: String,
    pub uri: String,
    pub committed: u64,
    pub ordinal: u64,
    /// Legacy file prefix digest, or a namespaced native snapshot change token.
    pub prefix_hash: String,
    pub continuation: String,
    pub generation: i64,
    pub missing: bool,
    pub size: Option<u64>,
    pub mtime_ns: Option<i64>,
    pub head: Option<u64>,
    pub boundary: Option<u64>,
    pub decoder_revision: u32,
}

/// Cheap inventory comparison, without loading decoder continuation state.
pub struct SourceStamp {
    pub key: String,
    pub uri: String,
    pub missing: bool,
    pub size: Option<u64>,
    pub mtime_ns: Option<i64>,
    pub source_token: String,
    pub decoder_revision: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    New,
    Append,
    Rewrite,
    Repair,
    Snapshot,
}

impl CaptureKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Append => "append",
            Self::Rewrite => "rewrite",
            Self::Repair => "repair",
            Self::Snapshot => "snapshot",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CaptureEncoding {
    Jsonl,
    ZstdJsonl,
    SqliteRows,
}

impl CaptureEncoding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::ZstdJsonl => "zstd-jsonl",
            Self::SqliteRows => "sqlite-rows-jsonl",
        }
    }
}

#[derive(Debug)]
pub struct PreparedCapture {
    pub expected: Option<SourceCheckpoint>,
    pub key: String,
    pub path: PathBuf,
    pub kind: CaptureKind,
    pub physical_cutoff: u64,
    pub mtime_ns: i64,
    pub encoding: CaptureEncoding,
    pub source_token: String,
    pub start: u64,
    pub start_ordinal: u64,
    pub end: u64,
    pub end_ordinal: u64,
    pub complete: bool,
    pub windows: (u64, u64),
    pub projection: ProjectionCheckpoint,
}

#[derive(Debug)]
pub struct ProjectionCheckpoint {
    pub session: SessionUpdate,
    pub continuation: String,
    pub decoder_revision: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreparedRecord {
    pub ordinal: u64,
    pub session: Option<SessionUpdate>,
    pub facts: Vec<Fact>,
    pub raw: Option<PreparedRaw>,
    pub document_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_raw_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreparedRaw {
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub leaves: Vec<ContentLeaf>,
    pub json_valid: bool,
}

#[derive(Debug)]
pub struct ReplaySource {
    pub profile: Profile,
    pub checkpoint: SourceCheckpoint,
    pub generation_id: i64,
}

#[derive(Debug)]
pub struct RetainedRecord {
    pub raw_id: Option<i64>,
    pub ordinal: u64,
    pub hash: String,
    pub bytes: Vec<u8>,
}

pub struct PreparedCatalog {
    pub previous: Option<String>,
    pub fingerprint: String,
    pub loaded: Option<(PathBuf, &'static str, crate::catalog::ProjectCatalog)>,
    pub threads: Option<Vec<crate::native::codex::ThreadMetadata>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CatalogReceipt {
    pub snapshots: usize,
    pub projects: usize,
    pub assignments: usize,
    pub errors: Vec<String>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateReceipt {
    pub imported: usize,
    pub appended: usize,
    pub unchanged: usize,
    pub missing: usize,
    pub records: usize,
    pub errors: Vec<String>,
    pub catalogs: CatalogReceipt,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ReprocessReceipt {
    pub dry_run: bool,
    pub sources: usize,
    pub records: usize,
    pub facts: BTreeMap<String, usize>,
    pub errors: Vec<String>,
}

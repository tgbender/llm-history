//! Archive operations and retained source query values.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveInfo {
    pub archive: PathBuf,
    pub identity: String,
    pub schema: String,
    pub bytes: u64,
    pub raw: Value,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct RestoreReceipt {
    pub restored: PathBuf,
    pub restored_from: PathBuf,
    pub archived_previous: Option<PathBuf>,
    pub verification: ArchiveInfo,
}
#[derive(Debug, Default)]
pub struct RawFilter {
    pub profiles: Vec<String>,
    pub source: Option<String>,
    pub selector: Option<String>,
    pub generation: Option<i64>,
    pub latest: bool,
}
#[derive(Debug, Serialize)]
pub struct RawSource {
    pub id: i64,
    pub profile: String,
    pub source: String,
    pub path: PathBuf,
    pub source_key: String,
    pub generation: i64,
    pub bytes: u64,
    pub lines: u64,
    pub missing: bool,
}

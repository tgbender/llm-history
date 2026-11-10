//! Shared session/record metadata values; native decoding lives in ingest.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Session {
    pub revision: u64,
    pub has_metadata: bool,
    pub runtime_id: String,
    pub kind: String,
    pub native_kind: String,
    pub evidence: String,
    pub started: String,
    pub repository_root: String,
    pub agent_label: String,
    #[serde(skip)]
    pub metadata: String,
    pub claude_metadata: serde_json::Map<String, Value>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub agent_protocol: bool,
    pub turn_id: String,
    pub arguments_json: Option<String>,
    pub native_type: String,
    pub payload_type: String,
    pub item_id: String,
    pub parent_id: String,
    pub metadata: String,
}

//! Search excerpts and ranked response values.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Attribution and bounded text selected for a search hit. Fetch complete text via context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageExcerpt {
    pub id: String,
    pub session_id: String,
    pub message_no: i64,
    pub session_no: i64,
    pub profile: String,
    pub source: String,
    pub thread_id: String,
    pub ts: String,
    pub role: String,
    pub phase: String,
    pub text: String,
    pub cwd: String,
    pub project: String,
    pub path: String,
    pub ordinal: u64,
    pub text_hash: String,
    pub subagent: bool,
    #[serde(default)]
    pub excluded: bool,
    #[serde(default)]
    pub session_seq: i64,
    #[serde(default)]
    pub context_epoch_id: Option<String>,
    #[serde(default)]
    pub conversation_group_id: Option<String>,
    #[serde(default)]
    pub authorship: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub message_kind: String,
    #[serde(default)]
    pub session_kind: String,
    #[serde(default)]
    pub native_item_id: Option<String>,
    #[serde(default)]
    pub timestamp_utc: Option<String>,
    #[serde(default)]
    pub timestamp_offset_minutes: Option<i32>,
    #[serde(default)]
    pub timestamp_timezone: Option<String>,
    #[serde(default)]
    pub timestamp_basis: Option<String>,
}
impl From<llm_history_types::query::Message> for MessageExcerpt {
    fn from(message: llm_history_types::query::Message) -> Self {
        Self {
            id: message.id,
            session_id: message.session_id,
            message_no: message.message_no,
            session_no: message.session_no,
            profile: message.profile,
            source: message.source,
            thread_id: message.thread_id,
            ts: message.ts,
            role: message.role,
            phase: message.phase,
            text: message.text,
            cwd: message.cwd,
            project: message.project,
            path: message.path,
            ordinal: message.ordinal,
            text_hash: message.text_hash,
            subagent: message.subagent,
            excluded: message.excluded,
            session_seq: message.session_seq,
            context_epoch_id: message.context_epoch_id,
            conversation_group_id: message.conversation_group_id,
            authorship: message.authorship,
            origin: message.origin,
            message_kind: message.message_kind,
            session_kind: message.session_kind,
            native_item_id: message.native_item_id,
            timestamp_utc: message.timestamp_utc,
            timestamp_offset_minutes: message.timestamp_offset_minutes,
            timestamp_timezone: message.timestamp_timezone,
            timestamp_basis: message.timestamp_basis,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub message: MessageExcerpt,
    pub score: f64,
    pub lexical_score: f64,
    pub semantic_score: f64,
    pub rerank_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<usize>,
    pub details: Value,
}

pub struct SimilarRequest {
    pub message_no: i64,
    pub dimensions: usize,
    pub filter: llm_history_types::query::Filter,
    pub limit: usize,
    pub per_session: usize,
    pub other_sessions: bool,
}

pub struct SimilarHit {
    pub message: MessageExcerpt,
    pub similarity: f64,
}

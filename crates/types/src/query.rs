//! Shared history selection, message and search request values.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub message_no: i64,
    #[serde(default)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub session_no: i64,
    pub profile: String,
    pub provider: String,
    pub thread_id: String,
    pub cwd: String,
    pub project: String,
    pub title: String,
    pub model: String,
    pub parent_thread_id: String,
    pub relation: String,
    pub subagent: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub effort: String,
    pub messages: u64,
    pub conversation_group_id: Option<String>,
    pub native_runtime_id: Option<String>,
    pub session_kind: String,
    pub native_session_kind: Option<String>,
    pub session_kind_evidence: String,
    pub agent_label: Option<String>,
    pub tool_calls: u64,
    pub compactions: u64,
    pub children: u64,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: String,
    pub profile: String,
    pub source: String,
    pub native_id: String,
    pub name: String,
    pub kind: String,
    pub metadata: Value,
    pub sessions: u64,
    pub roots: Vec<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsagePeriod {
    pub period: String,
    pub profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub input: i64,
    pub output: i64,
    pub cached: i64,
    pub cache_write: i64,
    pub thinking: i64,
    pub total: i64,
    pub uncached: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Filter {
    pub profiles: Vec<String>,
    pub source: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub role: Option<String>,
    pub threads: Vec<String>,
    pub cwd: Option<String>,
    pub under: Option<String>,
    pub project: Option<String>,
    pub include_subagents: bool,
    pub include_agent_protocol: bool,
    /// Only sessions of this kind: interactive, subagent, automation, guardian or unknown.
    pub kind: Option<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Auto,
    Lexical,
    Exact,
    Semantic,
    Hybrid,
}
impl std::str::FromStr for Mode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "auto" => Ok(Self::Auto),
            "lexical" => Ok(Self::Lexical),
            "exact" => Ok(Self::Exact),
            "semantic" => Ok(Self::Semantic),
            "hybrid" => Ok(Self::Hybrid),
            _ => bail!("expected auto, lexical, exact, semantic, or hybrid"),
        }
    }
}
#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub query: String,
    pub mode: Mode,
    pub filter: Filter,
    pub limit: usize,
    pub per_session: usize,
    pub snippet_chars: usize,
    pub max_output_chars: usize,
    pub full: bool,
    pub redact: bool,
    pub rerank_depth: usize,
    pub explain: bool,
}
impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: Mode::Lexical,
            filter: Filter {
                include_subagents: true,
                ..Filter::default()
            },
            limit: 5,
            per_session: 2,
            snippet_chars: 1000,
            max_output_chars: 5000,
            full: false,
            redact: true,
            rerank_depth: 25,
            explain: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Selection {
    pub profiles: Vec<String>,
    /// Harness key (`codex`, `claude`, `cursor`, or `all`), independent of native session kind.
    pub source: Option<String>,
}

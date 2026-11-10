//! Shared timestamp, context and token metadata values.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Timestamp {
    pub utc: Option<String>,
    pub offset_minutes: Option<i32>,
    pub timezone: String,
    pub basis: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextFact {
    pub cwd: String,
    pub current_date: String,
    pub timezone: String,
    pub model: String,
    pub effort: String,
    pub approval_policy: String,
    pub sandbox_policy: String,
    pub personality: String,
    pub service_tier: String,
    pub collaboration_mode_json: String,
    pub workspace_roots_json: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenValues {
    pub input: Option<i64>,
    pub cached: Option<i64>,
    pub cache_write: Option<i64>,
    pub output: Option<i64>,
    pub reasoning: Option<i64>,
    pub total: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenFact {
    pub ordinal: u64,
    pub context_line: Option<u64>,
    pub total: TokenValues,
    pub last: TokenValues,
    pub model_context_window: Option<i64>,
    pub limit_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Detail {
    pub raw_timestamp: String,
    pub time: Timestamp,
    pub turn_id: String,
    pub context: Option<ContextFact>,
    pub tokens: Option<TokenFact>,
}

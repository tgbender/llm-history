//! Bounded read-only SQL requests.
use llm_history_types::storage::DEFAULT_SQL_TIMEOUT_SECONDS;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlRequest {
    pub query: String,
    pub limit: usize,
    pub max_cell_bytes: usize,
    pub max_output_bytes: usize,
    pub redact: bool,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}
const fn default_timeout_seconds() -> u64 {
    DEFAULT_SQL_TIMEOUT_SECONDS
}

impl Default for SqlRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 50,
            max_cell_bytes: 2000,
            max_output_bytes: 8000,
            redact: true,
            timeout_seconds: default_timeout_seconds(),
        }
    }
}

//! Import requests and resolved profile selections.
use llm_history_types::{ingestion::UpdateReceipt, query::Selection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub selection: Selection,
    pub repair: bool,
}

#[derive(Debug, Serialize)]
pub struct UpdateOutcome {
    pub receipt: UpdateReceipt,
    /// Resolved enabled profiles, also used by a frontend's background scheduler.
    pub profiles: Vec<String>,
}

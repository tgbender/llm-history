//! Decoder continuation state. Serialized fields remain compatible with parser v6.
use llm_history_types::{
    Harness, canonical,
    ingestion::{ProjectionCheckpoint, SessionUpdate},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParserState {
    #[serde(default)]
    pub provider: ProviderState,
    #[serde(default)]
    pub canonical: canonical::Session,
    pub session: String,
    pub cwd: String,
    pub model: String,
    pub title: String,
    pub parent: String,
    pub relation: String,
    pub subagent: bool,
    /// Claude message IDs whose usage was counted. They are stored with their facts
    /// and reloaded on resume, as Python does, so the state does not grow with them.
    #[serde(skip)]
    pub seen_usage: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub turn_id: String,
    #[serde(default)]
    pub token_ordinal: u64,
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub has_context: bool,
    /// Claude title fallbacks, in Python's order, kept as `title_line`s: the state
    /// is rewritten on every append and a whole first message can run to tens of
    /// kilobytes.
    #[serde(default)]
    pub last_prompt: String,
    #[serde(default)]
    pub first_user_message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderState {
    pub ordinal: u64,
    pub timezone: String,
    pub context_line: Option<u64>,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub permission: String,
    pub service_tier: String,
}

impl ParserState {
    pub fn session_update(&self) -> SessionUpdate {
        SessionUpdate {
            canonical: self.canonical.clone(),
            timezone: self.provider.timezone.clone(),
            session: self.session.clone(),
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            title: self.title.clone(),
            parent: self.parent.clone(),
            relation: self.relation.clone(),
            subagent: self.subagent,
            last_prompt: self.last_prompt.clone(),
            first_user_message: self.first_user_message.clone(),
        }
    }

    pub fn projection_checkpoint(&self) -> serde_json::Result<ProjectionCheckpoint> {
        Ok(ProjectionCheckpoint {
            session: self.session_update(),
            continuation: serde_json::to_string(self)?,
            decoder_revision: crate::PARSER_VERSION,
        })
    }

    pub fn new(path: &Path, harness: Harness) -> Self {
        let normalized = path.to_string_lossy().replace('\\', "/");
        let path = Path::new(&normalized);
        match harness {
            Harness::Codex => crate::codex::state::new(path),
            Harness::Claude => crate::claude_code::state::new(path),
            Harness::Cursor => crate::cursor::state(path),
        }
    }
}

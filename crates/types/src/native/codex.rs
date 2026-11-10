//! Selected durable Codex thread metadata.
#[derive(Debug, Clone, Default)]
pub struct ThreadMetadata {
    pub id: String,
    pub rollout_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub source: String,
    pub cwd: String,
    pub title: String,
    pub archived: bool,
    pub first_user_message: String,
    pub model: String,
    pub reasoning_effort: String,
    pub thread_source: String,
    pub preview: String,
    pub name: String,
    pub agent_path: String,
    pub source_db: String,
}

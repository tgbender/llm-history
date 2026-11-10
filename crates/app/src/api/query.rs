//! Application query contracts. CLI aliases and formatting live in the CLI crate.
use llm_history_types::query::Filter;

#[derive(Debug, Clone)]
pub struct MessageQuery {
    pub filter: Filter,
    /// Exclusive opaque message-ID cursor from a previous page in this database.
    pub after: Option<String>,
    pub limit: usize,
}
impl Default for MessageQuery {
    fn default() -> Self {
        Self {
            filter: Filter::default(),
            after: None,
            limit: 100,
        }
    }
}

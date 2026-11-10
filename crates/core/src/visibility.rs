//! Shared session visibility under explicit query selections.
use llm_history_types::query::Filter;

/// Naming a kind asks for those sessions, hidden or not.
pub fn hides_subagents(filter: &Filter) -> bool {
    filter.kind.is_none() && !filter.include_subagents
}
/// Subagent and guardian sessions are mostly protocol, so naming them shows it.
pub fn hides_agent_protocol(filter: &Filter) -> bool {
    !filter.include_agent_protocol
        && !matches!(filter.kind.as_deref(), Some("subagent" | "guardian"))
}

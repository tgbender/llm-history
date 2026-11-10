//! Codex native identity and session metadata interpretation.
use crate::{ParserState, decode::string};
use llm_history_types::canonical;
use serde_json::Value;
use std::path::Path;

pub(crate) fn new(path: &Path) -> ParserState {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let native = stem
        .trim_start_matches("rollout-")
        .split('-')
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("-");
    ParserState {
        session: native.clone(),
        canonical: canonical::Session {
            runtime_id: native,
            ..Default::default()
        },
        ..Default::default()
    }
}

pub(crate) fn update_session(state: &mut canonical::Session, p: &Value) {
    state.revision += 1;
    state.has_metadata = true;
    state.runtime_id = string(p, "session_id");
    if state.runtime_id.is_empty() {
        state.runtime_id = string(p, "id");
    }
    state.started = string(p, "timestamp");
    state.repository_root = string(p, "repository_root");
    let spawn = &p["source"]["subagent"]["thread_spawn"];
    state.agent_label = p
        .get("agent_nickname")
        .or_else(|| p.get("agent_role"))
        .or_else(|| spawn.get("agent_nickname"))
        .or_else(|| spawn.get("agent_role"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into();
    let source = &p["source"];
    state.native_kind = p
        .get("thread_source")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if source.get("subagent").is_some() {
                format!(
                    "subagent:{}",
                    spawn
                        .get("agent_role")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )
            } else {
                source.as_str().unwrap_or("unknown").into()
            }
        });
    // Codex records approval reviews as subagents whose source is
    // {"subagent":{"other":"guardian"}}; they review a request, not delegated work.
    let guardian = source["subagent"]["other"] == "guardian";
    state.kind = if guardian || state.native_kind.contains("guardian") {
        "guardian"
    } else if source.get("subagent").is_some() || state.native_kind == "subagent" {
        "subagent"
    } else if matches!(
        state.native_kind.as_str(),
        "automation" | "batch" | "headless" | "workflow"
    ) {
        "automation"
    } else {
        "interactive"
    }
    .into();
    state.evidence = match state.kind.as_str() {
        "subagent" if state.native_kind == "subagent" => "native_thread_source",
        "subagent" => "adapter_source_metadata",
        "guardian" if guardian => "adapter_source_metadata",
        "automation" | "guardian" => "native_session_kind",
        _ => "adapter_default",
    }
    .into();
    state.metadata = p.to_string();
}

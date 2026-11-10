//! Claude native identity and session metadata interpretation.
use crate::{ParserState, decode::string};
use llm_history_types::canonical;
use serde_json::Value;
use std::path::Path;

pub(crate) fn new(path: &Path) -> ParserState {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let subagent = path.components().any(|p| p.as_os_str() == "subagents");
    let agent_id = if subagent {
        stem.trim_start_matches("agent-").to_owned()
    } else {
        String::new()
    };
    let native = if subagent {
        path.parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    } else {
        stem.into_owned()
    };
    let runtime = format!("claude:{native}");
    ParserState {
        session: if subagent {
            format!("{runtime}:{agent_id}")
        } else {
            runtime.clone()
        },
        parent: if subagent {
            runtime.clone()
        } else {
            String::new()
        },
        relation: if subagent {
            "spawned".into()
        } else {
            String::new()
        },
        canonical: canonical::Session {
            runtime_id: runtime,
            ..Default::default()
        },
        subagent,
        agent_id,
        ..Default::default()
    }
}

pub(crate) fn update_session(state: &mut canonical::Session, v: &Value, agent: &str) {
    let previous = state.clone();
    state.has_metadata = true;
    let native = v
        .get("sessionId")
        .or_else(|| v.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !native.is_empty() {
        state.runtime_id = format!("claude:{native}");
    }
    // Forked transcripts open with later bookkeeping records before the copied
    // conversation, so a Claude session starts at its earliest timestamp.
    let timestamp = string(v, "timestamp");
    if !timestamp.is_empty() && (state.started.is_empty() || timestamp < state.started) {
        state.started = timestamp;
    }
    state.kind = if agent.is_empty() {
        "interactive"
    } else {
        "subagent"
    }
    .into();
    state.native_kind = if agent.is_empty() {
        v.get("entrypoint")
            .and_then(Value::as_str)
            .unwrap_or("claude")
    } else {
        "subagent"
    }
    .into();
    state.evidence = if agent.is_empty() {
        "adapter_default"
    } else {
        "native_path"
    }
    .into();
    let mut metadata = Value::Object(state.claude_metadata.clone());
    for (key, target) in [
        ("sessionId", "session_id"),
        ("entrypoint", "entrypoint"),
        ("gitBranch", "git_branch"),
        ("permissionMode", "permission_mode"),
        ("slug", "slug"),
        ("userType", "user_type"),
    ] {
        if let Some(value) = v.get(key).or_else(|| {
            if key == "sessionId" {
                v.get("session_id")
            } else {
                None
            }
        }) {
            metadata[target] = value.clone();
        } else if metadata.get(target).is_none() {
            metadata[target] = Value::String(String::new());
        }
    }
    if metadata["session_id"].as_str().is_none_or(|s| s.is_empty()) {
        metadata["session_id"] = Value::String(
            state
                .runtime_id
                .strip_prefix("claude:")
                .unwrap_or(&state.runtime_id)
                .into(),
        );
    }
    metadata["agent_id"] = Value::String(if agent.is_empty() { "main" } else { agent }.into());
    state.metadata = metadata.to_string();
    state.claude_metadata = metadata.as_object().cloned().unwrap_or_default();
    state.native_kind = if agent.is_empty() {
        metadata
            .get("entrypoint")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("claude")
    } else {
        "subagent"
    }
    .into();
    state.agent_label = agent.into();
    if *state != previous {
        state.revision = previous.revision + 1;
    }
}

//! Native record, context and usage interpretation; no source or storage I/O.
use crate::{
    ParserState, ProviderState,
    decode::{content_text, count, string},
};
use llm_history_core::provider::timestamp;
use llm_history_types::{
    Fact, Usage, canonical,
    provider::{ContextFact, Detail, TokenFact, TokenValues},
};
use serde_json::Value;

pub(crate) fn decode(state: &mut ParserState, v: &Value, kind: &str, fact: &mut Fact) {
    let p = &v["payload"];
    match kind {
        "session_meta" if !state.canonical.has_metadata => {
            super::state::update_session(&mut state.canonical, p);
            let native = string(p, "id");
            if !native.is_empty() {
                state.session = native;
            }
            state.cwd = string(p, "cwd");
            state.parent = string(p, "forked_from_id");
            if state.parent.is_empty() {
                state.parent = string(p, "parent_session_id");
            }
            state.relation = "forked_from".into();
            if state.parent.is_empty() {
                state.parent = string(p, "parent_thread_id");
                state.relation = "continued_from".into();
            }
            if state.parent.is_empty() {
                state.relation.clear();
            }
            // Guardian reviews stay out of default listings, search and
            // embedding like other agent-driven sessions.
            state.subagent = matches!(state.canonical.kind.as_str(), "subagent" | "guardian");
            if state.subagent && !state.parent.is_empty() {
                state.relation = "spawned".into();
            }
            if state.parent.is_empty() {
                state.parent = string(&p["source"]["subagent"]["thread_spawn"], "parent_thread_id");
                if !state.parent.is_empty() {
                    state.relation = "spawned".into();
                }
            }
        }
        "turn_context" => {
            state.has_context = true;
            let turn = string(p, "turn_id");
            if !turn.is_empty() {
                state.turn_id = turn;
            }
            let effort = p
                .get("effort")
                .or_else(|| p["info"].get("effort"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if !effort.is_empty() {
                state.effort = effort;
            }
            let model = p
                .get("model")
                .or_else(|| p["info"].get("model"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if !model.is_empty() {
                state.model = model;
            }
            if state.cwd.is_empty() {
                state.cwd = string(p, "cwd");
            }
        }
        "response_item" => {
            fact.kind = string(p, "type");
            fact.role = string(p, "role");
            fact.phase = string(p, "phase");
            match fact.kind.as_str() {
                "message" | "agent_message" => {
                    if fact.kind == "agent_message" && fact.role.is_empty() {
                        fact.role = "agent".into();
                    }
                    fact.kind = "message".into();
                    fact.text = content_text(&p["content"]);
                }
                "function_call" | "custom_tool_call" | "mcp_tool_call" | "local_shell_call" => {
                    fact.kind = "tool_call".into();
                    fact.tool = p
                        .get("name")
                        .or_else(|| p.get("tool_name"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into();
                    fact.call_id = string(p, "call_id");
                    fact.text = p
                        .get("arguments")
                        .or_else(|| p.get("input"))
                        .or_else(|| p.get("action"))
                        .map(|x| {
                            x.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| x.to_string())
                        })
                        .unwrap_or_default();
                }
                "function_call_output"
                | "custom_tool_call_output"
                | "mcp_tool_call_output"
                | "local_shell_call_output" => {
                    fact.kind = "tool_result".into();
                    fact.call_id = string(p, "call_id");
                    fact.text = p
                        .get("output")
                        .or_else(|| p.get("content"))
                        .map(|x| {
                            x.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| x.to_string())
                        })
                        .unwrap_or_default();
                }
                _ => {}
            }
        }
        "event_msg" if p["type"] == "task_started" => {
            state.turn_id = p
                .get("turn_id")
                .or_else(|| p.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into();
            state.token_ordinal = 0;
        }
        "event_msg" if p["type"] == "token_count" => {
            fact.kind = "usage".into();
            state.token_ordinal += 1;
            if !state.turn_id.is_empty() {
                fact.usage_key = format!("{}:{}", state.turn_id, state.token_ordinal);
            }
            fact.usage_eligible = !state.subagent || !state.turn_id.is_empty() || state.has_context;
            let u = &p["info"]["last_token_usage"];
            fact.usage = Usage {
                input: count(u, "input_tokens"),
                output: count(u, "output_tokens"),
                cache_read: count(u, "cached_input_tokens"),
                cache_write: u
                    .get("cache_write_input_tokens")
                    .or_else(|| u.get("cache_creation_input_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                thinking: count(u, "reasoning_output_tokens"),
            };
        }
        "compacted" => {
            fact.kind = "compaction".into();
            fact.text = string(p, "message");
        }
        _ => {}
    }
}

pub(crate) fn detail(state: &mut ProviderState, v: &Value, f: &Fact, token_ordinal: u64) -> Detail {
    let kind = string(v, "type");
    let p = &v["payload"];
    let mut detail = Detail {
        raw_timestamp: f.timestamp.clone(),
        turn_id: f.canonical.turn_id.clone(),
        ..Default::default()
    };
    if kind == "turn_context" {
        let timezone = string(p, "timezone");
        if !timezone.is_empty() {
            state.timezone = timezone;
        }
        state.context_line = Some(state.ordinal);
        let sandbox = &p["sandbox_policy"];
        detail.context = Some(ContextFact {
            cwd: string(p, "cwd"),
            current_date: string(p, "current_date"),
            timezone: string(p, "timezone"),
            model: f.model.clone(),
            effort: f.effort.clone(),
            approval_policy: string(p, "approval_policy"),
            sandbox_policy: if sandbox.is_object() {
                string(sandbox, "type")
            } else {
                sandbox.as_str().unwrap_or_default().into()
            },
            personality: string(p, "personality"),
            service_tier: p
                .get("service_tier")
                .or_else(|| p.get("service_tier_name"))
                .or_else(|| p["info"].get("service_tier"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            collaboration_mode_json: p
                .get("collaboration_mode")
                .filter(|v| !v.is_null())
                .map(Value::to_string)
                .unwrap_or_default(),
            workspace_roots_json: p
                .get("workspace_roots")
                .filter(|v| !v.is_null())
                .map(Value::to_string)
                .unwrap_or_default(),
        });
    }
    detail.time = timestamp(&f.timestamp, &state.timezone);
    if kind == "event_msg" && p["type"] == "token_count" {
        detail.tokens = Some(TokenFact {
            ordinal: token_ordinal,
            context_line: state.context_line,
            total: token_values(&p["info"]["total_token_usage"]),
            last: token_values(&p["info"]["last_token_usage"]),
            model_context_window: p["info"]["model_context_window"].as_i64(),
            limit_id: string(&p["rate_limits"], "limit_id"),
        });
    }
    detail
}
fn token_values(v: &Value) -> TokenValues {
    let n = |key: &str| v.get(key).and_then(Value::as_i64);
    TokenValues {
        input: n("input_tokens"),
        cached: n("cached_input_tokens").or_else(|| n("cache_read_input_tokens")),
        cache_write: n("cache_write_input_tokens").or_else(|| n("cache_creation_input_tokens")),
        output: n("output_tokens"),
        reasoning: n("reasoning_output_tokens"),
        total: n("total_tokens"),
    }
}

pub(crate) fn excluded(fact: &Fact) -> bool {
    let lower = fact.text.trim_start().to_lowercase();
    lower.starts_with("<environment_context>")
        || lower.starts_with("# agents.md instructions")
        || lower.contains("<instructions>")
        || lower.starts_with("<codex_internal_context")
        || lower.starts_with("<subagent_notification>")
        || lower.starts_with("<turn_aborted>")
        || lower.starts_with("<external_codex_apps_")
        || lower.starts_with("<realtime_delegation>")
}

pub(crate) fn record_metadata(v: &Value) -> canonical::Record {
    let payload = &v["payload"];
    let keys = payload
        .as_object()
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let payload_type = string(payload, "type");
    canonical::Record {
        turn_id: payload
            .get("internal_chat_message_metadata_passthrough")
            .and_then(|m| m.get("turn_id"))
            .or_else(|| payload.get("turn_id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        arguments_json: if matches!(
            payload_type.as_str(),
            "function_call" | "custom_tool_call" | "mcp_tool_call" | "local_shell_call"
        ) {
            payload
                .get("arguments")
                .or_else(|| payload.get("input"))
                .or_else(|| payload.get("action"))
                .map(Value::to_string)
        } else {
            None
        },
        native_type: string(v, "type"),
        payload_type,
        item_id: string(payload, "id"),
        parent_id: string(v, "parentUuid"),
        metadata: serde_json::to_string(&keys).unwrap_or_default(),
        ..Default::default()
    }
}

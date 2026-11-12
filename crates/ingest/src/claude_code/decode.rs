//! Native record, context and usage interpretation; no source or storage I/O.
use crate::{
    ParserState, ProviderState,
    decode::{content_text, count, string},
};
use llm_history_core::{provider::timestamp, title_line};
use llm_history_types::{
    Fact, Usage, canonical,
    provider::{ContextFact, Detail, TokenFact, TokenValues},
};
use serde_json::Value;

pub(crate) fn decode(
    state: &mut ParserState,
    v: &Value,
    kind: &str,
    fact: &mut Fact,
    facts: &mut Vec<Fact>,
) -> bool {
    // Request logs contain replayed turns and must remain archive-only events.
    if matches!(
        kind,
        "api-request" | "api-request-blob" | "api-request-shape"
    ) {
        fact.kind = "request_log".into();
        fact.detail = detail(&mut state.provider, v, fact, state.token_ordinal);
        return true;
    }
    let native = v
        .get("sessionId")
        .or_else(|| v.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    super::state::update_session(&mut state.canonical, v, &state.agent_id);
    if !native.is_empty() {
        state.session = if state.agent_id.is_empty() {
            format!("claude:{native}")
        } else {
            format!("claude:{native}:{}", state.agent_id)
        };
        if !state.agent_id.is_empty() {
            state.parent = format!("claude:{native}");
            state.relation = "spawned".into();
        }
    }
    let cwd = string(v, "cwd");
    if !cwd.is_empty() {
        state.cwd = cwd;
    }
    if let Some(title) = v.get("summary").and_then(Value::as_str) {
        state.title = title.to_owned();
    }
    for title_key in ["aiTitle", "customTitle"] {
        if let Some(title) = v.get(title_key).and_then(Value::as_str) {
            state.title = title.into();
        }
    }
    let prompt = string(v, "lastPrompt");
    if kind == "last-prompt" && !prompt.is_empty() {
        state.last_prompt = title_line(&prompt);
    }
    if kind == "user" && state.first_user_message.is_empty() {
        state.first_user_message = title_line(&content_text(&v["message"]["content"]));
    }
    let parent = string(v, "parentSessionId");
    if !parent.is_empty() {
        state.parent = format!("claude:{parent}");
        state.relation = "parent".into();
    }
    state.subagent = !state.agent_id.is_empty();
    let m = &v["message"];
    let model = string(m, "model");
    if !model.is_empty() {
        state.model = model;
    }
    let effort = string(v, "effort");
    if !effort.is_empty() {
        state.effort = effort;
    }
    if kind == "assistant" || kind == "user" {
        fact.kind = "message".into();
        fact.role = kind.to_owned();
        fact.text = content_text(&m["content"]);
        if kind == "assistant" && !fact.text.is_empty() {
            fact.phase = if matches!(
                m["stop_reason"].as_str(),
                Some("end_turn" | "stop_sequence")
            ) {
                "final_answer"
            } else {
                "commentary"
            }
            .into();
        }
        let u = &m["usage"];
        let message_id = m
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| v.get("uuid").and_then(Value::as_str));
        if u.is_object() && message_id.is_some_and(|id| state.seen_usage.insert(id.to_owned())) {
            state.token_ordinal += 1;
            fact.usage_key = message_id.unwrap_or_default().into();
            fact.usage = Usage {
                input: count(u, "input_tokens")
                    + count(u, "cache_read_input_tokens")
                    + count(u, "cache_creation_input_tokens"),
                output: count(u, "output_tokens"),
                cache_read: count(u, "cache_read_input_tokens"),
                cache_write: count(u, "cache_creation_input_tokens"),
                thinking: count(&u["output_tokens_details"], "thinking_tokens"),
            };
        }
        if let Some(blocks) = m["content"].as_array() {
            for (i, block) in blocks.iter().enumerate() {
                if matches!(block["type"].as_str(), Some("tool_use" | "tool_result")) {
                    let call = block["type"] == "tool_use";
                    facts.push(Fact {
                        index: i + 1,
                        kind: if call { "tool_call" } else { "tool_result" }.into(),
                        text: if call {
                            block["input"].to_string()
                        } else {
                            content_text(&block["content"])
                        },
                        tool: string(block, "name"),
                        call_id: string(block, if call { "id" } else { "tool_use_id" }),
                        usage: Usage::default(),
                        ..(*fact).clone()
                    });
                }
            }
        }
    }
    if kind == "system" && v["subtype"] == "compact_boundary" {
        fact.kind = "compaction".into();
    }

    false
}

pub(crate) fn detail(state: &mut ProviderState, v: &Value, f: &Fact, token_ordinal: u64) -> Detail {
    let kind = string(v, "type");
    let mut detail = Detail {
        raw_timestamp: f.timestamp.clone(),
        turn_id: f.canonical.turn_id.clone(),
        ..Default::default()
    };
    if !matches!(
        kind.as_str(),
        "api-request" | "api-request-blob" | "api-request-shape"
    ) {
        let mode = if kind == "mode" {
            string(v, "mode")
        } else {
            state.mode.clone()
        };
        let permission = if kind == "permission-mode" {
            string(v, "permissionMode")
        } else {
            state.permission.clone()
        };
        let service = v["message"]["usage"]
            .get("service_tier")
            .and_then(Value::as_str)
            .unwrap_or(&state.service_tier)
            .to_owned();
        if state.model != f.model
            || state.effort != f.effort
            || state.mode != mode
            || state.permission != permission
            || state.service_tier != service
        {
            state.context_line = Some(state.ordinal);
            detail.context = Some(ContextFact {
                cwd: string(v, "cwd"),
                model: f.model.clone(),
                effort: f.effort.clone(),
                approval_policy: permission.clone(),
                sandbox_policy: mode.clone(),
                service_tier: service.clone(),
                ..Default::default()
            });
        }
        state.model = f.model.clone();
        state.effort = f.effort.clone();
        state.mode = mode;
        state.permission = permission;
        state.service_tier = service;
    }
    detail.time = timestamp(&f.timestamp, &state.timezone);
    if !f.usage_key.is_empty() {
        detail.turn_id = f.usage_key.clone();
        let usage = token_values(&f.usage);
        detail.tokens = Some(TokenFact {
            ordinal: token_ordinal,
            context_line: state.context_line,
            last: usage,
            ..Default::default()
        });
    }
    detail
}
fn token_values(usage: &Usage) -> TokenValues {
    TokenValues {
        input: Some(usage.input),
        cached: Some(usage.cache_read),
        cache_write: Some(usage.cache_write),
        output: Some(usage.output),
        reasoning: Some(usage.thinking),
        total: Some(usage.input + usage.output),
    }
}

/// Tags Claude Code puts around text it writes into a user record.
const HARNESS_TAGS: [&str; 3] = [
    "system-reminder",
    "local-command-stdout",
    "task-notification",
];

pub(crate) fn excluded(v: &Value, fact: &Fact) -> bool {
    v["isMeta"].as_bool().unwrap_or(false)
        || (fact.role == "user" && only_harness_blocks(&fact.text))
}

/// True when the text is nothing but harness-written blocks. A reminder followed by
/// what the user typed stays a user message.
fn only_harness_blocks(text: &str) -> bool {
    let mut rest = text.trim();
    let mut blocks = 0;
    'blocks: while !rest.is_empty() {
        for tag in HARNESS_TAGS {
            let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
            if let Some(body) = rest.strip_prefix(&open)
                && let Some(end) = body.find(&close)
            {
                rest = body[end + close.len()..].trim_start();
                blocks += 1;
                continue 'blocks;
            }
        }
        return false;
    }
    blocks > 0
}

pub(crate) fn record_metadata(v: &Value) -> canonical::Record {
    let payload = &v["message"];
    let keys = v
        .as_object()
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let payload_type = if let Some(blocks) = payload["content"].as_array() {
        if blocks.len() == 1 {
            string(&blocks[0], "type")
        } else {
            string(v, "subtype")
        }
    } else {
        string(v, "subtype")
    };
    canonical::Record {
        turn_id: payload
            .get("internal_chat_message_metadata_passthrough")
            .and_then(|m| m.get("turn_id"))
            .or_else(|| payload.get("turn_id"))
            .or_else(|| payload.get("id"))
            .or_else(|| v.get("uuid"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        native_type: string(v, "type"),
        payload_type,
        item_id: string(v, "uuid"),
        parent_id: string(v, "parentUuid"),
        metadata: serde_json::to_string(&keys).unwrap_or_default(),
        ..Default::default()
    }
}

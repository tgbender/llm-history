//! Pure decoding of Cursor JSONL and retained CLI message rows.
use crate::{
    ParserState,
    decode::{content_text, string},
};
use llm_history_core::{provider::timestamp, title_line};
use llm_history_types::{Fact, canonical, provider::Detail};
use serde_json::Value;

pub(super) const STORE_META: &str = "cursor_store_meta";

fn first_string<'a>(values: impl IntoIterator<Item = &'a Value>) -> &'a str {
    values
        .into_iter()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .unwrap_or("")
}

fn time(value: &Value) -> String {
    if let Some(ms) = value.as_i64() {
        return chrono::DateTime::from_timestamp_millis(ms)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();
    }
    value.as_str().unwrap_or_default().into()
}

fn message(v: &Value) -> &Value {
    v.get("message").unwrap_or(v)
}

pub(crate) fn record_metadata(v: &Value) -> canonical::Record {
    canonical::Record {
        native_type: first_string([&v["type"], &v["role"]]).into(),
        payload_type: first_string([&message(v)["role"], &v["role"]]).into(),
        item_id: string(message(v), "id"),
        ..Default::default()
    }
}

pub(crate) fn detail(fact: &Fact) -> Detail {
    Detail {
        raw_timestamp: fact.timestamp.clone(),
        time: timestamp(&fact.timestamp, ""),
        ..Default::default()
    }
}

fn update_metadata(state: &mut ParserState, v: &Value) {
    let (meta, sidecar) = (&v["meta"], &v["sidecar"]);
    state.title = first_string([&sidecar["title"], &meta["name"]]).into();
    state.cwd = first_string([&sidecar["cwd"], &meta["workspacePath"]]).into();
    state.model = first_string([&meta["lastUsedModel"]["modelId"], &meta["lastUsedModel"]]).into();
    state.subagent |= sidecar["isSubagent"] == true;
    state.canonical.started = time(sidecar.get("createdAtMs").unwrap_or(&meta["createdAt"]));
    state.canonical.has_metadata = true;
    state.canonical.revision += 1;
}

fn tool(fact: &Fact, index: usize, block: &Value, role: &str) -> Option<Fact> {
    let call = match block["type"].as_str() {
        Some("tool_use" | "tool-call") => true,
        Some("tool_result" | "tool-result") => false,
        _ => return None,
    };
    let data = if call {
        block.get("input").or_else(|| block.get("args"))
    } else {
        block
            .get("result")
            .or_else(|| block.get("output"))
            .or_else(|| block.get("content"))
            .or_else(|| block.get("error"))
    };
    let text = data
        .filter(|v| !v.is_null())
        .map(|v| match v {
            Value::String(s) => s.clone(),
            _ => v.to_string(),
        })
        .unwrap_or_default();
    let phase = if !call && (block["isError"] == true || !block["error"].is_null()) {
        "error"
    } else {
        ""
    };
    Some(Fact {
        index,
        kind: if call { "tool_call" } else { "tool_result" }.into(),
        role: role.into(),
        phase: phase.into(),
        tool: first_string([&block["toolName"], &block["name"]]).into(),
        call_id: first_string([
            &block["toolCallId"],
            &block["tool_call_id"],
            &block["tool_use_id"],
            &block["id"],
        ])
        .into(),
        canonical: canonical::Record {
            arguments_json: if call {
                data.map(Value::to_string)
            } else {
                None
            },
            ..fact.canonical.clone()
        },
        text,
        ..fact.clone()
    })
}

pub(crate) fn decode(state: &mut ParserState, v: &Value, fact: &mut Fact, extra: &mut Vec<Fact>) {
    if v["type"] == STORE_META {
        update_metadata(state, v);
        return;
    }
    let m = message(v);
    let role = first_string([&m["role"], &v["role"]]);
    fact.timestamp = time(
        m.get("createdAt")
            .or_else(|| v.get("timestamp"))
            .unwrap_or(&Value::Null),
    );
    let content = &m["content"];
    if let Some(blocks) = content.as_array() {
        extra.extend(
            blocks
                .iter()
                .enumerate()
                .filter_map(|(i, b)| tool(fact, i + 1, b, role)),
        );
    }
    if let Some(calls) = m["tool_calls"].as_array() {
        for call in calls {
            let function = &call["function"];
            let args = function["arguments"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| function["arguments"].clone());
            let block = serde_json::json!({
                "type": "tool-call", "name": function["name"], "id": call["id"], "args": args
            });
            extra.extend(tool(
                fact,
                extra.len() + content.as_array().map_or(1, |a| a.len() + 1),
                &block,
                role,
            ));
        }
    }
    if role == "tool" && extra.is_empty() {
        let block = serde_json::json!({
            "type": "tool-result", "name": m["name"], "tool_call_id": m["tool_call_id"],
            "result": content, "isError": m["isError"], "error": m["error"]
        });
        extra.extend(tool(fact, 1, &block, role));
    }
    let mut text = content_text(content);
    if matches!(role, "user" | "assistant") && !text.trim().is_empty() {
        // CLI stores a separate synthetic user-context leaf.
        fact.excluded = role == "user" && text.trim_start().starts_with("<user_info>");
        if role == "user"
            && let Some(query) = text
                .trim()
                .strip_prefix("<user_query>")
                .and_then(|s| s.strip_suffix("</user_query>"))
        {
            text = query.trim().into();
        }
        if role == "user" && !fact.excluded && state.first_user_message.is_empty() {
            state.first_user_message = title_line(&text);
        }
        fact.kind = "message".into();
        fact.role = role.into();
        fact.text = text;
        if role == "assistant" {
            fact.phase = if extra.is_empty() {
                "final_answer"
            } else {
                "commentary"
            }
            .into();
        }
    } else if !extra.is_empty() {
        *fact = extra.remove(0);
        fact.index = 0;
    }
}

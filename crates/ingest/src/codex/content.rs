use crate::content::{Leaves, Object};

pub(crate) fn extract<'a>(reader: &mut Leaves<'a>, root: &Object<'a>) -> serde_json::Result<()> {
    let Some(payload) = root.get("payload") else {
        return response(reader, root, "");
    };
    match root.kind() {
        Some("response_item" | "event_msg" | "compacted") if payload.get().starts_with('{') => {
            let payload: Object<'a> = serde_json::from_str(payload.get())?;
            match root.kind() {
                Some("response_item") => response(reader, &payload, "/payload")?,
                Some("event_msg")
                    if matches!(payload.kind(), Some("user_message" | "agent_message")) =>
                {
                    reader.text(payload.get("message"), "/payload/message".into());
                }
                Some("compacted") => {
                    reader.text(payload.get("message"), "/payload/message".into());
                    reader.blocks(
                        payload.get("replacement_history"),
                        "/payload/replacement_history",
                        |this, item, path| response(this, &item, path),
                    )?;
                }
                _ => {}
            }
        }
        _ => {}
    }
    Ok(())
}

fn response<'a>(reader: &mut Leaves<'a>, item: &Object<'a>, path: &str) -> serde_json::Result<()> {
    let field = match item.kind() {
        Some("function_call") => "arguments",
        Some("custom_tool_call") => "input",
        Some("message") => "content",
        Some("function_call_output" | "custom_tool_call_output") => "output",
        _ => return Ok(()),
    };
    let at = format!("{path}/{field}");
    if matches!(field, "content" | "output") {
        reader.blocks(item.get(field), &at, |this, block, path| {
            if matches!(block.kind(), Some("input_text" | "output_text")) {
                this.text(block.get("text"), format!("{path}/text"));
            }
            Ok(())
        })?;
    } else {
        reader.text(item.get(field), at);
    }
    Ok(())
}

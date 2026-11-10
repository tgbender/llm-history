use crate::content::{Leaves, Object};

pub(crate) fn extract<'a>(reader: &mut Leaves<'a>, root: &Object<'a>) -> serde_json::Result<()> {
    let Some(raw) = root.get("message").filter(|raw| raw.get().starts_with('{')) else {
        return Ok(());
    };
    if !matches!(root.kind(), Some("user" | "assistant")) {
        return Ok(());
    }
    let message: Object<'a> = serde_json::from_str(raw.get())?;
    reader.blocks(
        message.get("content"),
        "/message/content",
        |this, block, path| {
            match block.kind() {
                Some("text") => this.text(block.get("text"), format!("{path}/text")),
                Some("tool_result") => this.blocks(
                    block.get("content"),
                    &format!("{path}/content"),
                    |this, block, path| {
                        if block.kind() == Some("text") {
                            this.text(block.get("text"), format!("{path}/text"));
                        }
                        Ok(())
                    },
                )?,
                _ => {}
            }
            Ok(())
        },
    )
}

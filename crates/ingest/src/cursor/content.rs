use crate::content::{Leaves, Object};

pub(crate) fn extract<'a>(reader: &mut Leaves<'a>, root: &Object<'a>) -> serde_json::Result<()> {
    let Some(raw) = root.get("message") else {
        return Ok(());
    };
    if !raw.get().starts_with('{') {
        return Ok(());
    }
    let message: Object<'a> = serde_json::from_str(raw.get())?;
    reader.blocks(
        message.get("content"),
        "/message/content",
        |reader, block, path| {
            for name in match block.kind() {
                Some("text") => &["text"][..],
                Some("tool-result" | "tool_result") => {
                    &["result", "output", "content", "error"][..]
                }
                _ => &[],
            } {
                reader.text(block.get(name), format!("{path}/{name}"));
            }
            Ok(())
        },
    )
}

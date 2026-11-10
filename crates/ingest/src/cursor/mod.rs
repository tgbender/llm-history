//! Cursor native transcript and CLI chat-store adapter.
pub(crate) mod content;
pub(crate) mod decode;
pub(crate) mod source;
mod wire;

use crate::ParserState;
use llm_history_types::canonical;
use std::path::Path;

pub(crate) fn state(key: &Path) -> ParserState {
    let native = key.file_stem().unwrap_or_default().to_string_lossy();
    let parent = key
        .parent()
        .filter(|p| p.ends_with("subagents"))
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|id| format!("cursor:{}", id.to_string_lossy()));
    ParserState {
        session: format!("cursor:{native}"),
        subagent: parent.is_some(),
        relation: parent
            .as_ref()
            .map_or_else(String::new, |_| "spawned".into()),
        parent: parent.unwrap_or_default(),
        canonical: canonical::Session {
            runtime_id: native.into_owned(),
            ..Default::default()
        },
        ..Default::default()
    }
}

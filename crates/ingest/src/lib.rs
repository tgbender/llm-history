//! Ingestion adapters for native coding-agent histories.
//!
//! Each harness owns a module for its native format and compatibility handling.
//! Shared JSON primitives preserve unfamiliar data while known-field decoding
//! stays explicit. Codex and Claude decoding and continuation state are used by
//! live import and archive replay. Other harnesses currently have envelope readers.
//! Capture mechanics, persistence, CAS identity and workflows have separate owners.

pub mod catalog;
pub mod claude_code;
pub mod codex;
pub mod content;
mod cursor;
pub mod opencode;
pub mod pi;
pub mod prepare;
pub mod source;
pub mod tau;

mod decode;
mod json;
mod presence;
mod state;

#[cfg(test)]
mod decode_tests;

/// Existing checkpoint/projection revision, retained across this ownership move.
pub const PARSER_VERSION: u32 = 7;

pub use decode::content_text;
pub use json::{FieldError, JsonRecord};
pub use presence::Presence;
pub use state::{ParserState, ProviderState};

//! Native Claude Code transcript readers.
//!
//! Open envelope readers preserve unknown JSON. The internal decoder and state
//! modules supply the established import/replay behavior without source I/O.
//! See the repository's docs/harness-history/claude-code/ reference for native semantics.

pub mod catalog;
pub(crate) mod content;
pub(crate) mod decode;
mod record;
pub(crate) mod state;

pub use record::Record;

//! Native Codex transcript readers.
//!
//! Open envelope readers preserve unknown JSON. The internal decoder and state
//! modules supply the established import/replay behavior without source I/O.
//! See the repository's docs/harness-history/codex/ reference for native semantics.
//!
//! ```
//! use llm_history_ingest::{Presence, codex::Record};
//!
//! let source = r#"{"type":"response_item","payload":{"type":"future.item"},"extra":true}"#;
//! let record = Record::parse(source)?;
//! let Presence::Value(payload) = record.payload()? else { panic!("payload object") };
//! assert_eq!(payload.field::<String>("type")?, Presence::Value("future.item".into()));
//! assert_eq!(record.native().source(), source);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod catalog;
pub(crate) mod content;
pub(crate) mod decode;
mod record;
pub mod source;
pub(crate) mod state;

pub use record::Record;

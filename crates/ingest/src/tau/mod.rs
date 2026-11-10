//! Native Hugging Face Tau transcript readers.
//!
//! The initial API reads open native envelopes and preserves unknown JSON. Full
//! content normalization can migrate here incrementally; acquisition stays outside.
//! See the repository's docs/harness-history/tau/ reference for native semantics.

mod record;

pub use record::Record;

//! Native Pi transcript readers.
//!
//! The initial API reads open native envelopes and preserves unknown JSON. Full
//! content normalization can migrate here incrementally; acquisition stays outside.
//! See the repository's docs/harness-history/pi/ reference for native semantics.

mod record;

pub use record::Record;

//! Shared history identity, fact and metadata values.
//!
//! Native interpretation, acquisition, search policy and persistence have separate
//! owners. These types retain the existing serialization contract during extraction.

pub mod archive;
pub mod canonical;
pub mod catalog;
mod fact;
pub mod ingestion;
pub mod jobs;
pub mod native;
mod profile;
pub mod provider;
pub mod query;
pub mod storage;
mod usage;

pub use fact::Fact;
pub use profile::{Harness, Profile};
pub use usage::Usage;

//! Storage-owned SQL fault injection and persistence invariants.
mod archives;
mod canonical;
mod catalogs;
mod checkpoints;
mod compressed;
mod content_archive;
mod inspection;
mod transcripts;
use crate::Store;
#[path = "../../../../../fixtures/prepared_transcripts.rs"]
mod fixtures;
#[cfg(feature = "native-tests")]
mod native;
mod pending;
mod profiles;
mod provider;
mod publication;
mod recovery;
mod reprocess;
mod update;
mod usage;

mod embedding_publication;
mod native_transactions;
mod schema_readiness;

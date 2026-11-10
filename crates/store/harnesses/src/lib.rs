//! Native persistence modules share the transaction supplied by the history store.
//! This crate never opens source databases or commits a caller's transaction.
pub mod catalog;
pub mod codex;
pub mod schema;

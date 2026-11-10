//! Native OpenCode transcript row readers, independent of SQLite connections.
//!
//! Acquisition supplies a table name and JSON object of native columns. SQL TEXT
//! is represented as a JSON string, including JSON encoded inside a TEXT column.
//! Unknown tables and columns survive. See docs/harness-history/opencode/.

mod row;

pub use row::Row;

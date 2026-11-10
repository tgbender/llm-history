//! Native schema components applied by the history store's migration plan.
use anyhow::Result;
use duckdb::Connection;

pub fn apply(connection: &Connection) -> Result<()> {
    connection.execute_batch(include_str!("codex/schema.sql"))?;
    connection.execute_batch(include_str!("catalog.sql"))?;
    Ok(())
}

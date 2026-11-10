//! Initialize the development schema. Existing development databases are converted offline.
use super::*;

impl Store {
    pub(crate) fn require_schema(&self) -> Result<String> {
        let (version, numbers): (String, bool) = self
            .database
            .connection()
            .query_row(
                "SELECT value, (SELECT count(*) = 2 FROM duckdb_tables() \
                 WHERE database_name=current_database() AND schema_name='main' \
                 AND table_name IN ('message_numbers','session_numbers')) \
                 FROM meta WHERE key='schema_version'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .context("database has no llm-history schema")?;
        anyhow::ensure!(
            version == SCHEMA_VERSION,
            "development schema {version}; convert a copy manually or choose a new --db"
        );
        anyhow::ensure!(
            numbers,
            "archive lacks short IDs; backfill a copy offline or choose a new --db"
        );
        Ok(version)
    }

    pub(crate) fn initialize(&self) -> Result<()> {
        llm_history_store_engine::verify_linked_engine()?;
        let con = self.database.connection();
        let has_meta: bool = con.query_row(
            "SELECT EXISTS(SELECT 1 FROM duckdb_tables() WHERE database_name=current_database() AND schema_name='main' AND table_name='meta')",
            [], |r| r.get(0),
        )?;
        if has_meta {
            self.require_schema()?;
            return Ok(());
        }
        let tx = self.database.begin()?;
        con.execute_batch(include_str!("sql/base.sql"))?;
        con.execute_batch(include_str!("sql/numbers.sql"))?;
        con.execute_batch(include_str!("sql/content.sql"))?;
        llm_history_store_harnesses::schema::apply(con)?;
        con.execute_batch(include_str!("sql/canonical.sql"))?;
        con.execute_batch(include_str!("sql/event_views.sql"))?;
        con.execute_batch(include_str!("sql/provider.sql"))?;
        con.execute_batch(&format!(
            "CREATE OR REPLACE VIEW usage_events AS {}",
            crate::usage::events_query(false)
        ))?;
        con.execute_batch(include_str!("sql/views.sql"))?;
        con.execute(
            "INSERT INTO meta VALUES ('schema_version',?),('identity',?)",
            params![SCHEMA_VERSION, uuid::Uuid::new_v4().to_string()],
        )?;
        tx.commit()
    }
}

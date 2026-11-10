//! Typed storage capabilities for index/schema readiness and refresh scheduling.
use super::*;
use llm_history_types::jobs::IndexReceipt;

impl Store {
    pub fn ensure_search_index(&self) -> Result<()> {
        let present: bool = self.database.connection().query_row(
            "SELECT count(*)>0 FROM information_schema.schemata WHERE schema_name='fts_main_lexical_documents'",
            [], |row| row.get(0),
        )?;
        if !present {
            self.create_fts()?;
        }
        Ok(())
    }

    pub fn index(&self, rebuild: bool) -> Result<IndexReceipt> {
        if rebuild {
            self.create_fts()?;
        }
        let documents = self.database.connection().query_row(
            "SELECT count(*) FROM lexical_documents",
            [],
            |row| row.get(0),
        )?;
        Ok(IndexReceipt {
            documents,
            changed: rebuild,
            rebuilt: rebuild,
            mode: "incremental".into(),
        })
    }

    /// A successful refresh does not leave a cooldown marker.
    pub fn refresh_failure_at(&self, profile: &str) -> Option<i64> {
        self.database
            .connection()
            .query_row(
                "select value from meta where key=?",
                [format!("refresh_failure:{profile}")],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|value| value.parse().ok())
    }

    pub fn set_refresh_failure(&self, profile: &str, timestamp: Option<i64>) -> Result<()> {
        let key = format!("refresh_failure:{profile}");
        let con = self.database.connection();
        if let Some(timestamp) = timestamp {
            con.execute(
                "insert into meta values (?,?) on conflict(key) do update set value=excluded.value",
                [key, timestamp.to_string()],
            )?;
        } else {
            con.execute("delete from meta where key=?", [key])?;
        }
        Ok(())
    }

    /// An interrupted import may have committed facts but not their derived metadata.
    pub fn refresh_projection_pending(&self, profile: &str) -> Result<bool> {
        Ok(self.database.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key IN (?,?))",
            [
                format!("project_pending:{profile}"),
                format!("conversation_graph_pending:{profile}"),
            ],
            |row| row.get(0),
        )?)
    }
}

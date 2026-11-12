//! Create the schema, upgrade an older database, and keep views and macros current.
//!
//! A database records two things in `meta`. `schema_version` is the table layout and
//! moves only through [`MIGRATIONS`]. `definitions` is a digest of every view and macro;
//! those hold no data, so a build whose definitions differ replaces them.
use super::*;

/// Table-layout upgrades. Entry N-1 takes a database from version N to N+1. An entry is
/// never edited once merged, so any database can follow the steps in order.
const MIGRATIONS: [&str; (SCHEMA_VERSION - 1) as usize] = [
    // 1 to 2 changes no table. The history views began giving the short numbers as
    // session_id and message_id, which a build for version 1 would misread as keys.
    "",
];

/// The database is older than this build expects; a writable open upgrades it.
#[derive(Debug)]
pub(crate) struct UpgradeNeeded;
impl std::fmt::Display for UpgradeNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "database needs an upgrade by this build")
    }
}
impl std::error::Error for UpgradeNeeded {}

/// Every view and macro, in dependency order.
fn definitions() -> String {
    [
        include_str!("sql/base_views.sql"),
        include_str!("sql/event_views.sql"),
        include_str!("sql/provider_views.sql"),
        &format!(
            "CREATE OR REPLACE VIEW usage_events AS {};",
            crate::usage::events_query(false)
        ),
        include_str!("sql/views.sql"),
    ]
    .join("\n")
}

struct Recorded {
    version: u32,
    definitions: Option<String>,
}

impl Store {
    fn recorded(&self) -> Result<Recorded> {
        let (version, definitions, numbers): (String, Option<String>, bool) = self
            .database
            .connection()
            .query_row(
                "SELECT value, (SELECT value FROM meta WHERE key='definitions'), \
                 (SELECT count(*) = 2 FROM duckdb_tables() \
                 WHERE database_name=current_database() AND schema_name='main' \
                 AND table_name IN ('message_numbers','session_numbers')) \
                 FROM meta WHERE key='schema_version'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .context("database has no llm-history schema")?;
        let version: u32 = version
            .parse()
            .with_context(|| format!("unrecognized schema version {version}"))?;
        anyhow::ensure!(
            (1..=SCHEMA_VERSION).contains(&version),
            "database schema {version} is newer than this build supports ({SCHEMA_VERSION}); \
             upgrade llm-history"
        );
        anyhow::ensure!(
            numbers,
            "archive lacks short IDs; backfill a copy offline or choose a new --db"
        );
        Ok(Recorded {
            version,
            definitions,
        })
    }

    /// The schema version of a database this build can read or upgrade.
    pub(crate) fn require_schema(&self) -> Result<String> {
        Ok(self.recorded()?.version.to_string())
    }

    /// Fails with [`UpgradeNeeded`] unless the tables and definitions are this build's.
    pub(crate) fn require_current(&self) -> Result<()> {
        let recorded = self.recorded()?;
        let current = blake3::hash(definitions().as_bytes()).to_hex().to_string();
        if recorded.version < SCHEMA_VERSION || recorded.definitions.as_deref() != Some(&current) {
            return Err(UpgradeNeeded.into());
        }
        Ok(())
    }

    pub(crate) fn initialize(&self) -> Result<()> {
        llm_history_store_engine::verify_linked_engine()?;
        let con = self.database.connection();
        let has_meta: bool = con.query_row(
            "SELECT EXISTS(SELECT 1 FROM duckdb_tables() WHERE database_name=current_database() AND schema_name='main' AND table_name='meta')",
            [], |r| r.get(0),
        )?;
        let definitions = definitions();
        let current = blake3::hash(definitions.as_bytes()).to_hex().to_string();
        let recorded = has_meta.then(|| self.recorded()).transpose()?;
        if recorded.as_ref().is_some_and(|recorded| {
            recorded.version == SCHEMA_VERSION && recorded.definitions.as_deref() == Some(&current)
        }) {
            return Ok(());
        }
        let tx = self.database.begin()?;
        match &recorded {
            None => {
                con.execute_batch(include_str!("sql/base.sql"))?;
                con.execute_batch(include_str!("sql/numbers.sql"))?;
                con.execute_batch(include_str!("sql/content.sql"))?;
                llm_history_store_harnesses::schema::apply(con)?;
                con.execute_batch(include_str!("sql/canonical.sql"))?;
                con.execute_batch(include_str!("sql/provider.sql"))?;
                con.execute(
                    "INSERT INTO meta VALUES ('schema_version',?),('identity',?)",
                    params![SCHEMA_VERSION.to_string(), uuid::Uuid::new_v4().to_string()],
                )?;
            }
            Some(recorded) if recorded.version < SCHEMA_VERSION => {
                for step in &MIGRATIONS[(recorded.version - 1) as usize..] {
                    con.execute_batch(step)?;
                }
                con.execute(
                    "UPDATE meta SET value=? WHERE key='schema_version'",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(_) => {}
        }
        con.execute_batch(&definitions)?;
        con.execute(
            "INSERT OR REPLACE INTO meta VALUES ('definitions',?)",
            params![current],
        )?;
        tx.commit()
    }
}

//! DuckDB persistence. Every native identity is scoped by a durable profile/store.
use anyhow::{Context, Result, bail};
use duckdb::{Connection, params};
use llm_history_core::paths::database_path;
use llm_history_core::{bounded, code_aliases, project_name};
use llm_history_types::{
    archive::{ArchiveInfo, RawFilter, RawSource, RestoreReceipt},
    ingestion::{CatalogReceipt, PreparedRecord, SessionUpdate},
    query::{Filter, Message},
    storage::StoreOptions,
};

use llm_history_store_engine::{Database, DatabaseLock};
use llm_history_types::{Fact, Harness, Profile};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: &str = "1";
mod archive_data;
pub mod archives;
mod batch;
mod canonical;
mod catalogs;
mod embedding_batch;
mod embeddings;
mod metadata;
mod pending;
mod provider;
mod publication;
mod queries;
mod raw;
mod reprocess;
mod retrieval;
mod schema;
mod session_batch;
mod sql_output;
mod status;
mod usage;
use batch::Batch;
pub use embedding_batch::{EmbeddingBatch, EmbeddingDocument, EmbeddingState};
pub use pending::PendingPage;

/// Serializes archive ingestion across short-lived database connections.
pub struct IngestionLock {
    _lock: DatabaseLock,
}

pub struct Store {
    database: Database,
    pub path: PathBuf,
    read_only: bool,
    options: StoreOptions,
}
fn float_list(v: &[f32]) -> duckdb::types::Value {
    duckdb::types::Value::List(v.iter().copied().map(duckdb::types::Value::Float).collect())
}

/// A conversation message is the physical record of a projected fact.
pub(crate) const MESSAGE_RECORD: &str =
    "e.fact_index=0 AND e.session_seq IS NOT NULL AND e.role<>''";
const MESSAGE_COLUMNS: &str = "e.id,e.session_id,p.name,s.native_id,e.ts,e.role,e.phase,\
                             e.text,s.cwd,s.project,src.uri,e.ordinal,e.text_hash,s.subagent,\
                             {harness},e.excluded,e.session_seq,e.context_epoch_id,\
                             gm.conversation_group_id,history_authorship(e.excluded,e.role,\
                             cs.session_kind),history_origin(e.excluded,e.role,\
                             cs.session_kind),history_message_kind(e.kind,e.excluded,e.text),\
                             cs.session_kind,e.native_item_id,strftime(e.timestamp_utc,\
                             '%Y-%m-%dT%H:%M:%S.%gZ'),e.timestamp_offset_minutes,\
                             e.timestamp_timezone,e.timestamp_basis,mn.message_no,sn.session_no";
const MESSAGE_FROM: &str = "events e \
                             join canonical_sessions cs on cs.id=e.session_id and cs.active \
                             join sessions s on s.id=cs.id \
                             join profiles p on p.id=s.profile_id \
                             join sources src on src.id=e.source_id \
                             left join canonical_conversation_memberships gm on gm.session_id=cs.id and gm.is_primary";
fn message_query(harness: &str) -> String {
    let columns = MESSAGE_COLUMNS.replace("{harness}", harness);
    format!(
        "SELECT {columns} FROM {MESSAGE_FROM} \
             JOIN message_numbers mn ON mn.event_id=e.id \
             JOIN session_numbers sn ON sn.session_id=s.id"
    )
}
fn message_row(r: &duckdb::Row<'_>) -> duckdb::Result<Message> {
    Ok(Message {
        id: r.get::<_, i64>(0)?.to_string(),
        session_id: r.get::<_, i64>(1)?.to_string(),
        message_no: r.get(28)?,
        session_no: r.get(29)?,
        profile: r.get(2)?,
        thread_id: r.get(3)?,
        ts: r.get(4)?,
        role: r.get(5)?,
        phase: r.get(6)?,
        text: r.get(7)?,
        cwd: r.get(8)?,
        project: r.get(9)?,
        path: r.get(10)?,
        ordinal: r.get(11)?,
        text_hash: r.get(12)?,
        subagent: r.get(13)?,
        source: r.get(14)?,
        excluded: r.get(15)?,
        session_seq: r.get(16)?,
        context_epoch_id: r.get::<_, Option<i64>>(17)?.map(|v| v.to_string()),
        conversation_group_id: r.get::<_, Option<i64>>(18)?.map(|v| v.to_string()),
        authorship: r.get(19)?,
        origin: r.get(20)?,
        message_kind: r.get(21)?,
        session_kind: r.get(22)?,
        native_item_id: r.get(23)?,
        timestamp_utc: r.get(24)?,
        timestamp_offset_minutes: r.get(25)?,
        timestamp_timezone: r.get(26)?,
        timestamp_basis: r.get(27)?,
    })
}

impl Store {
    pub fn open(path: &Path, read_only: bool) -> Result<Self> {
        Self::open_with_options(path, read_only, StoreOptions::default())
    }
    pub fn open_with_options(path: &Path, read_only: bool, options: StoreOptions) -> Result<Self> {
        Self::open_with_wait(path, read_only, options, std::time::Duration::ZERO)
    }
    /// Foreground callers can wait through brief reader/writer contention.
    /// Workers use nonblocking opens to retain their own cancellation/deadline policy.
    pub fn lock_ingestion(path: &Path, timeout: std::time::Duration) -> Result<IngestionLock> {
        Ok(IngestionLock {
            _lock: DatabaseLock::acquire_ingestion(&database_path(path)?, timeout)?,
        })
    }

    pub fn open_with_wait(
        path: &Path,
        read_only: bool,
        options: StoreOptions,
        timeout: std::time::Duration,
    ) -> Result<Self> {
        if options.memory_mib < 1 || options.batch_records == 0 || options.batch_bytes == 0 {
            bail!("memory and batch limits must be positive");
        }
        if read_only && !path.is_file() {
            bail!("Database does not exist: {}", path.display());
        }
        let path = database_path(path)?;
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let lock = DatabaseLock::acquire(&path, read_only, timeout)?;
        Self::open_locked_options(&path, read_only, Some(lock), true, options)
    }
    // Used by archive operations while their caller holds the stable destination lock.
    fn open_locked(
        path: &Path,
        read_only: bool,
        lock: Option<DatabaseLock>,
        initialize: bool,
    ) -> Result<Self> {
        Self::open_locked_options(path, read_only, lock, initialize, StoreOptions::default())
    }
    fn open_locked_options(
        path: &Path,
        read_only: bool,
        lock: Option<DatabaseLock>,
        initialize: bool,
        options: StoreOptions,
    ) -> Result<Self> {
        let database = Database::open_locked(path, read_only, options, lock)?;
        let store = Self {
            database,
            options,
            path: path.to_owned(),
            read_only,
        };
        if initialize {
            if read_only {
                store.require_schema()?;
            } else {
                store.initialize()?;
            }
        }
        Ok(store)
    }
    pub fn identity(&self) -> Result<String> {
        Ok(self.database.connection().query_row(
            "select value from meta where key='identity'",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn next_id(&self) -> Result<i64> {
        Ok(self
            .database
            .connection()
            .query_row("select nextval('entity_ids')", [], |r| r.get(0))?)
    }
    pub fn profiles(&self, names: &[String], enabled_only: bool) -> Result<Vec<Profile>> {
        let mut st = self
            .database
            .connection()
            .prepare("select data from profiles order by name")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let p: Profile = serde_json::from_str(&row?)?;
            if (!enabled_only || p.enabled) && (names.is_empty() || names.contains(&p.name)) {
                out.push(p);
            }
        }
        for n in names {
            if !out.iter().any(|p| &p.name == n) {
                bail!("unknown or disabled profile: {n}");
            }
        }
        Ok(out)
    }
    pub fn add_profile(
        &self,
        name: &str,
        harness: Harness,
        root: &Path,
        account: Option<String>,
        backend: Option<String>,
    ) -> Result<Profile> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || "-_".contains(c))
        {
            bail!("profile names use letters, digits, - or _");
        }
        let root = root.canonicalize().context("profile root must exist")?;
        let all = self.profiles(&[], false)?;
        if all.iter().any(|p| p.name == name || p.root == root) {
            bail!("profile name or canonical root already registered");
        }
        let p = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            harness,
            root,
            account,
            backend,
            enabled: true,
        };
        self.save_profile(&p)?;
        Ok(p)
    }
    pub fn save_profile(&self, p: &Profile) -> Result<()> {
        self.database.connection().execute(
            "insert into profiles \
             values (?,?,?) on conflict(id) \
             do update set name=excluded.name,data=excluded.data",
            params![p.id, p.name, serde_json::to_string(p)?],
        )?;
        Ok(())
    }
    pub fn relocate(&self, name: &str, root: &Path) -> Result<Profile> {
        let mut p = self.profiles(&[name.into()], false)?.remove(0);
        let root = root.canonicalize()?;
        if self
            .profiles(&[], false)?
            .iter()
            .any(|other| other.id != p.id && other.root == root)
        {
            bail!("root already registered");
        }
        p.root = root;
        self.save_profile(&p)?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests;

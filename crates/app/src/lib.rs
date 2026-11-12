//! Public history application operations, independent of the command-line frontend.
//!
//! [`History`] is a lightweight configuration handle. It retains no database lock;
//! every operation opens and closes its own connection. See `docs/application-api.md`.
#![doc = include_str!("../../../docs/application-api.md")]
use anyhow::{Context, Result, bail};
use llm_history_store::{IngestionLock, Store};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub mod api;
mod ingestion;
mod refresh;
mod search;
use api::{
    ingestion::{UpdateOutcome, UpdateRequest},
    query::MessageQuery,
    sql::SqlRequest,
};
use llm_history_models::Embedder;
use llm_history_search::WorkBudget;
use llm_history_types::{
    Harness, Profile,
    archive::{ArchiveInfo, RawFilter, RawSource},
    ingestion::{CatalogReceipt, ReprocessReceipt},
    jobs::WorkReceipt,
    query::{
        Filter, Message, ProjectSummary, Selection, SessionSummary, SessionTurns, UsagePeriod,
    },
    storage::StoreOptions,
};

/// Explicit profile registration; no environment or home-directory lookup occurs here.
#[derive(Debug, Clone)]
pub struct ProfileRegistration {
    pub name: String,
    pub harness: Harness,
    pub root: PathBuf,
    pub account: Option<String>,
    pub backend: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HistoryOptions {
    pub database: PathBuf,
    pub storage: StoreOptions,
    pub lock_timeout: Duration,
    pub default_profiles: Vec<ProfileRegistration>,
}
impl HistoryOptions {
    pub fn new(database: impl Into<PathBuf>) -> Self {
        Self {
            database: database.into(),
            storage: StoreOptions::default(),
            lock_timeout: Duration::from_secs(5),
            default_profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct History {
    options: HistoryOptions,
}
impl History {
    /// Construct without I/O, profile discovery, downloads or background processes.
    pub fn new(database: impl Into<PathBuf>) -> Self {
        Self::with_options(HistoryOptions::new(database))
    }
    pub fn with_options(options: HistoryOptions) -> Self {
        Self { options }
    }
    pub fn database(&self) -> &Path {
        &self.options.database
    }
    fn open(&self, read_only: bool) -> Result<Store> {
        let store = Store::open_with_wait(
            self.database(),
            read_only,
            self.options.storage,
            self.options.lock_timeout,
        )?;
        if !read_only {
            store.ensure_search_index()?;
        }
        Ok(store)
    }
    fn lock_ingestion(&self) -> Result<IngestionLock> {
        Store::lock_ingestion(self.database(), self.options.lock_timeout)
    }

    fn defaults(&self, store: &Store) -> Result<()> {
        if store.profiles(&[], false)?.is_empty() {
            for p in &self.options.default_profiles {
                if p.root.is_dir() {
                    store.add_profile(
                        &p.name,
                        p.harness,
                        &p.root,
                        p.account.clone(),
                        p.backend.clone(),
                    )?;
                }
            }
        }
        Ok(())
    }
    pub fn initialize(&self) -> Result<Value> {
        let store = self.open(false)?;
        self.defaults(&store)?;
        store.status(&[], None, llm_history_ingest::PARSER_VERSION)
    }
    /// Create a store without applying configured default profile candidates.
    pub fn create(&self) -> Result<()> {
        self.open(false)?;
        Ok(())
    }
    pub fn profiles(&self, names: &[String], enabled_only: bool) -> Result<Vec<Profile>> {
        self.open(true)?.profiles(names, enabled_only)
    }
    pub fn register_profile(&self, p: &ProfileRegistration) -> Result<Profile> {
        let _ingestion = self.lock_ingestion()?;
        self.open(false)?.add_profile(
            &p.name,
            p.harness,
            &p.root,
            p.account.clone(),
            p.backend.clone(),
        )
    }
    pub fn set_profile_enabled(&self, name: &str, enabled: bool) -> Result<Profile> {
        let _ingestion = self.lock_ingestion()?;
        let store = self.open(false)?;
        let mut p = store.profiles(&[name.into()], false)?.remove(0);
        p.enabled = enabled;
        store.save_profile(&p)?;
        Ok(p)
    }
    pub fn relocate_profile(&self, name: &str, root: &Path) -> Result<Profile> {
        let _ingestion = self.lock_ingestion()?;
        self.open(false)?.relocate(name, root)
    }
    pub fn update(&self, request: &UpdateRequest) -> Result<UpdateOutcome> {
        let _ingestion = self.lock_ingestion()?;
        let store = self.open(false)?;
        self.defaults(&store)?;
        let mut profiles = request.selection.profiles.clone();
        if let Some(source) = &request.selection.source {
            profiles = store
                .profiles(&profiles, true)?
                .into_iter()
                .filter(|p| source == "all" || p.harness.key() == source)
                .map(|p| p.name)
                .collect();
            if profiles.is_empty() {
                bail!("no profiles match source");
            }
        }
        drop(store);
        let receipt = ingestion::update(self, &profiles, request.repair)?;
        Ok(UpdateOutcome { receipt, profiles })
    }
    pub fn status(&self, selection: &Selection) -> Result<Value> {
        self.open(true)?.status(
            &selection.profiles,
            selection.source.as_deref(),
            llm_history_ingest::PARSER_VERSION,
        )
    }
    pub fn sessions(
        &self,
        filter: &Filter,
        limit: usize,
        oldest: bool,
    ) -> Result<Vec<SessionSummary>> {
        self.open(true)?.list_sessions(filter, limit, oldest)
    }
    pub fn projects(
        &self,
        filter: &Filter,
        limit: usize,
        include_inactive: bool,
    ) -> Result<Vec<ProjectSummary>> {
        self.open(true)?
            .list_projects_including(filter, limit, include_inactive)
    }
    pub fn refresh_projects(&self, selection: &Selection) -> Result<CatalogReceipt> {
        let _ingestion = self.lock_ingestion()?;
        let profiles = self
            .open(false)?
            .profiles(&selection.profiles, true)?
            .into_iter()
            .filter(|p| {
                selection
                    .source
                    .as_deref()
                    .is_none_or(|s| s == "all" || s == p.harness.key())
            })
            .map(|p| p.name)
            .collect::<Vec<_>>();
        if profiles.is_empty() {
            bail!("no profiles match source");
        }
        ingestion::refresh_catalogs(self, &profiles)
    }
    /// Complete normalized message text. Search excerpts have their own type.
    pub fn messages(&self, query: &MessageQuery) -> Result<Vec<Message>> {
        if !(1..=1000).contains(&query.limit) {
            bail!("message page limit must be 1..=1000");
        }
        let after = query
            .after
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("invalid message ID cursor")?;
        self.open(true)?
            .messages_page(&query.filter, after, query.limit)
    }
    pub fn context(
        &self,
        message_no: i64,
        before: usize,
        after: usize,
        profiles: &[String],
    ) -> Result<Vec<Message>> {
        self.open(true)?
            .context(message_no, before, after, profiles)
    }
    /// One session's user turns in conversation order, with its last final answer.
    pub fn session_turns(&self, session_no: i64, profiles: &[String]) -> Result<SessionTurns> {
        self.open(true)?.session_turns(session_no, profiles)
    }
    pub fn usage(
        &self,
        filter: &Filter,
        zone: &str,
        weekly: bool,
        by_model: bool,
        limit: usize,
    ) -> Result<Vec<UsagePeriod>> {
        self.open(true)?
            .usage(filter, zone, weekly, by_model, limit)
    }
    /// Documented read-only views and macros; limits apply to serialized output.
    pub fn sql(&self, request: &SqlRequest) -> Result<Value> {
        self.open(true)?.sql_bounded(
            &request.query,
            request.limit,
            request.max_cell_bytes,
            request.max_output_bytes,
            request.redact,
            Duration::from_secs(request.timeout_seconds),
        )
    }
    pub fn reprocess(
        &self,
        selection: &Selection,
        source_ids: &[i64],
        dry_run: bool,
    ) -> Result<ReprocessReceipt> {
        if !self.database().is_file() {
            bail!("database does not exist");
        }
        let _ingestion = (!dry_run).then(|| self.lock_ingestion()).transpose()?;
        ingestion::reprocess(
            self,
            &selection.profiles,
            selection.source.as_deref(),
            source_ids,
            dry_run,
        )
    }
    pub fn raw_sources(&self, filter: &RawFilter) -> Result<Vec<RawSource>> {
        self.open(true)?.raw_sources(filter)
    }
    pub fn raw_stats(&self, filter: &RawFilter) -> Result<Value> {
        self.open(true)?.raw_stats_filtered(filter)
    }
    pub fn verify_raw(
        &self,
        filter: &RawFilter,
        against_live: bool,
        verbose: bool,
    ) -> Result<Value> {
        self.open(true)?.verify_raw(filter, against_live, verbose)
    }
    pub fn export_raw(
        &self,
        source: i64,
        generation: Option<i64>,
        out: &mut impl Write,
    ) -> Result<u64> {
        self.open(true)?.export_raw(source, generation, out)
    }
    pub fn snapshot(&self, output: &Path) -> Result<ArchiveInfo> {
        self.open(false)?.snapshot(output)
    }
    pub fn compact(&self) -> Result<Value> {
        if !self.database().is_file() {
            bail!("database does not exist");
        }
        self.open(false)?.compact()
    }
    pub fn index(&self, force: bool) -> Result<llm_history_types::jobs::IndexReceipt> {
        self.open(false)?.index(force)
    }
    pub fn coverage(&self, model: &str, profiles: &[String]) -> Result<Value> {
        self.open(true)?.coverage(model, profiles)
    }
    pub fn has_pending_messages(
        &self,
        model: &str,
        profiles: &[String],
        source: Option<&str>,
    ) -> Result<bool> {
        self.open(true)?
            .has_pending_messages(model, profiles, source)
    }
    pub fn backfill(
        &self,
        queue: &Path,
        engine: &mut dyn Embedder,
        budget: &WorkBudget,
    ) -> Result<WorkReceipt> {
        llm_history_search::backfill(self.database(), self.options.storage, queue, engine, budget)
    }
}

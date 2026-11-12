use anyhow::{Context, Result, bail};
use clap::Parser;
use llm_history_app::{History, HistoryOptions, ProfileRegistration};
use llm_history_core::bounded;
use llm_history_types::Harness;
use llm_history_types::query::Filter;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
mod args;
mod background;
mod commands;
mod engine;
mod output;
mod presentation;
mod raw;
mod search;
mod semantic;
mod show;
use args::*;
use background::*;
use engine::*;
use presentation::emit;

static CANCELLATION: std::sync::OnceLock<llm_history_models::CancellationToken> =
    std::sync::OnceLock::new();
static STORE_OPTIONS: std::sync::OnceLock<llm_history_types::storage::StoreOptions> =
    std::sync::OnceLock::new();
fn cancellation() -> llm_history_models::CancellationToken {
    CANCELLATION.get_or_init(Default::default).clone()
}

/// Where a command reads and writes, and how it prints.
pub(crate) struct Ctx {
    db: PathBuf,
    cache: PathBuf,
    json: bool,
}
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
fn db_path(app: &App) -> PathBuf {
    app.db.clone().unwrap_or_else(|| {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/share"))
            .join("llm-history-rs/history.duckdb")
    })
}
fn cache_path(app: &App) -> PathBuf {
    app.cache.clone().unwrap_or_else(|| {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".cache"))
            .join("llm-history-rs")
    })
}
fn store_options() -> llm_history_types::storage::StoreOptions {
    STORE_OPTIONS.get().copied().unwrap_or_default()
}

fn history(db: &Path) -> History {
    let mut options = HistoryOptions::new(db);
    options.storage = store_options();
    options.lock_timeout = Duration::from_secs(5);
    options.default_profiles = [
        ("codex", Harness::Codex, "CODEX_HOME"),
        ("claude", Harness::Claude, "CLAUDE_CONFIG_DIR"),
    ]
    .into_iter()
    .map(|(name, harness, var)| ProfileRegistration {
        name: name.into(),
        harness,
        root: std::env::var_os(var)
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(format!(".{name}"))),
        account: None,
        backend: None,
    })
    .collect();
    History::with_options(options)
}
impl Ctx {
    fn history(&self) -> History {
        history(&self.db)
    }
}
fn expand_home(path: String) -> String {
    if path == "~" {
        home().to_string_lossy().into_owned()
    } else if let Some(suffix) = path.strip_prefix("~/") {
        home().join(suffix).to_string_lossy().into_owned()
    } else {
        path
    }
}
fn time_bound(value: &Option<String>, zone: &str) -> Result<Option<String>> {
    llm_history_core::validate_timezone(zone)?;
    value
        .as_ref()
        .map(|v| llm_history_core::timestamp(v, zone))
        .transpose()
}
fn execute(app: App) -> Result<()> {
    let _ = STORE_OPTIONS.set(llm_history_types::storage::StoreOptions {
        memory_mib: app.memory_mib,
        batch_records: app.batch_records,
        batch_bytes: app
            .batch_mib
            .checked_mul(1024 * 1024)
            .context("batch size overflow")?,
    });
    let ctx = Ctx {
        db: llm_history_core::paths::database_path(&db_path(&app))?,
        cache: cache_path(&app),
        json: app.json,
    };
    match app.command {
        Commands::Init => commands::init(&ctx),
        Commands::Profile(command) => commands::profile(&ctx, command),
        Commands::Update {
            repair,
            profile,
            source,
            no_background,
            inference,
            ..
        } => commands::update(&ctx, repair, profile, source, no_background, &inference),
        Commands::Similar(args) => search::similar(&ctx, args),
        Commands::Search(args) | Commands::Semantic(Semantic::Search(args)) => {
            search::search(&ctx, *args)
        }
        Commands::Show(args) => show::show(&ctx, args),
        Commands::Status(selection) => commands::status(&ctx, selection),
        Commands::Sessions(args) => commands::sessions(&ctx, args),
        Commands::Projects(args) => commands::projects(&ctx, args),
        Commands::Reprocess(args) => commands::reprocess(&ctx, args),
        Commands::Usage(args) => commands::usage(&ctx, args),
        Commands::Sql(args) => commands::sql(&ctx, args),
        Commands::Db(command) => commands::db(&ctx, command),
        Commands::Raw(command) => raw::raw(&ctx, command),
        Commands::Archive(command) => raw::archive(&ctx, command),
        Commands::Models(command) => commands::models(&ctx, command),
        Commands::Semantic(command) => semantic::semantic(&ctx, command),
    }
}
fn main() {
    let token = cancellation();
    if let Err(error) = ctrlc::set_handler(move || token.cancel()) {
        eprintln!("error: cannot install cancellation handler: {error}");
        std::process::exit(1);
    }
    if let Err(error) = execute(App::parse()) {
        eprintln!("error: {}", llm_history_core::redact(&format!("{error:#}")));
        std::process::exit(if cancellation().is_cancelled() {
            130
        } else {
            1
        });
    }
}

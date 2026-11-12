//! Command-line arguments.
use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};
use clap::{ArgAction, Args, Parser, Subcommand};
use llm_history_types::Harness;
use llm_history_types::query::Mode;
use std::path::PathBuf;

const SESSION_KINDS: [&str; 5] = [
    "interactive",
    "subagent",
    "automation",
    "guardian",
    "unknown",
];
fn search_modes() -> impl TypedValueParser<Value = Mode> {
    PossibleValuesParser::new([
        PossibleValue::new("words")
            .help("Every word, as a case-insensitive string anywhere in the message, newest first"),
        PossibleValue::new("lexical").help("Any of the words, best match first"),
        PossibleValue::new("exact").help(
            "The whole query as one literal, case-insensitive string, newest first, \
             showing the matched message rather than its answer",
        ),
        PossibleValue::new("semantic").help("Similar meaning, using embeddings"),
        PossibleValue::new("hybrid").help("Lexical and semantic combined"),
        PossibleValue::new("auto")
            .help("Hybrid when cached embeddings are available, otherwise lexical"),
    ])
    .try_map(|mode| mode.parse::<Mode>())
}
const SOURCE_KEYS: [&str; 4] = ["all", "codex", "claude", "cursor"];

/// Flags that mean nothing to a command that only reads, accepted so a habit from
/// `search` doesn't fail it.
#[derive(Args, Debug, Default)]
pub(crate) struct ReadOnlyHabits {
    #[arg(long = "no-refresh", hide = true)]
    _no_refresh: bool,
    #[arg(long = "no-background", hide = true)]
    _no_background: bool,
}

#[derive(Parser)]
#[command(
    name = "llm-history",
    version,
    disable_help_flag = true,
    about = "Local coding-agent history with lossless archives and profile-scoped search",
    after_help = "Read history:\n  llm-history search 'words you remember typing'\n  llm-history search --mode semantic 'what went wrong?'\n  llm-history show ID ID\n  llm-history show --session SESSION_ID\n  llm-history similar ID --role user --other-sessions --per-session 1\n\nUse --no-refresh --no-background to search a fixed archive. Use plain text for reading and --json for parsing. Open context before counting related hits as separate incidents."
)]
pub(crate) struct App {
    /// Print help with the common options; --help-all lists every option.
    #[arg(short = 'h', long = "help", global = true, action = ArgAction::HelpShort)]
    help: Option<bool>,
    /// Print help with every option.
    #[arg(long = "help-all", global = true, action = ArgAction::HelpLong)]
    help_all: Option<bool>,
    /// DuckDB memory ceiling in MiB.
    #[arg(long, global = true, default_value_t = 1024, hide_short_help = true)]
    pub(crate) memory_mib: u64,
    /// Maximum uncompressed data per ingestion batch, in MiB.
    #[arg(long, global = true, default_value_t = 64, hide_short_help = true)]
    pub(crate) batch_mib: usize,
    /// Maximum records per ingestion write batch.
    #[arg(long, global = true, default_value_t = 20000, hide_short_help = true)]
    pub(crate) batch_records: usize,
    /// Database path; defaults to ~/.local/share/llm-history-rs/history.duckdb.
    #[arg(long, global = true, env = "LLM_HISTORY_RS_DB")]
    pub(crate) db: Option<PathBuf>,
    /// Directory for the llama.cpp runtime; models use the Hugging Face cache.
    #[arg(
        long,
        global = true,
        env = "LLM_HISTORY_RS_CACHE",
        hide_short_help = true
    )]
    pub(crate) cache: Option<PathBuf>,
    /// Print JSON for programmatic use.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[command(subcommand)]
    pub(crate) command: Commands,
}
#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Create the database and discover the default Codex and Claude roots.
    Init,
    #[command(subcommand)]
    /// Add, list, enable, disable or relocate independent history profiles.
    Profile(Profiles),
    /// Import new transcript records and refresh native project/session metadata.
    Update {
        /// Reimport live sources even when their metadata is unchanged.
        #[arg(long)]
        repair: bool,
        /// Select a history profile; repeat to select several.
        #[arg(long)]
        profile: Vec<String>,
        /// Select codex, claude, cursor, or all history sources.
        #[arg(long, value_parser = SOURCE_KEYS)]
        source: Option<String>,
        /// Skip automatic background embedding work.
        #[arg(long, overrides_with = "_background")]
        no_background: bool,
        /// Python's explicit default; overrides an earlier --no-background.
        #[arg(long = "background", hide = true, overrides_with = "no_background")]
        _background: bool,
        #[command(flatten)]
        inference: Inference,
    },
    /// Search conversation messages using lexical, semantic, hybrid or exact matching.
    #[command(
        before_help = "Examples:\n  llm-history search 'words you typed'\n  llm-history search --mode exact 'git push --force' --since 2026-10-01\n  llm-history search --in tools 'cargo clippy'"
    )]
    Search(Box<Search>),
    /// Find messages similar to an indexed message using its stored embeddings.
    Similar(SimilarArgs),
    /// Show messages by ID with their surrounding conversation, or read a session's turns.
    #[command(
        before_help = "Examples:\n  llm-history show 31305 31470\n  llm-history show 31305 -C 4\n  llm-history show --session 601 --recent 5"
    )]
    Show(ShowArgs),
    /// List recent sessions, including titles, models and project metadata.
    #[command(mut_arg("limit", |arg| arg.default_value("30")))]
    Sessions(SessionArgs),
    /// List discovered projects or refresh their native assignments.
    Projects(ProjectArgs),
    /// Rebuild parsed facts from retained archives while preserving message IDs.
    Reprocess(ReprocessArgs),
    /// Summarize deduplicated token usage by day or week.
    #[command(mut_arg("limit", |arg| arg.default_value(None::<&str>).help("Limit usage periods; omit to include all periods")))]
    Usage(UsageArgs),
    /// Show source coverage, entity counts and timestamp ranges.
    Status(ProfileSelection),
    /// Query history SQL views with bounded, read-only results.
    #[command(
        before_help = "Examples:\n  llm-history sql 'select * from history_help'\n  llm-history sql \"select column_name, description from history_dictionary where surface = 'history_messages'\"\n  llm-history sql 'select session_id, project_name, last_event_at_utc from history_sessions order by last_event_at_utc desc limit 10'"
    )]
    Sql(SqlArgs),
    #[command(subcommand)]
    /// Inspect or export the original captured transcript bytes.
    Raw(Raw),
    #[command(subcommand)]
    /// Create, list, verify or restore database snapshots.
    Archive(Archive),
    #[command(subcommand)]
    /// List, explicitly download or verify the local Gemma and Qwen models.
    Models(Models),
    #[command(subcommand)]
    /// Manage search indexes and resumable background embedding work.
    Semantic(Semantic),
    #[command(subcommand)]
    /// Maintain the Rust history database.
    Db(Database),
}
#[derive(Args)]
pub(crate) struct ShowArgs {
    /// Short message IDs from search or similar; up to 10 share the output budget.
    #[arg(
        value_parser = clap::value_parser!(i64).range(1..),
        num_args = 1..=10,
        required_unless_present = "session",
        conflicts_with = "session"
    )]
    pub(crate) id: Vec<i64>,
    /// Read one session's user turns in order, then its last answer. Takes the short
    /// session ID from sessions or a search hit. A turn over 1,000 characters keeps its
    /// start and end, and a session over the budget keeps its opening and latest turns.
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
    pub(crate) session: Option<i64>,
    /// With --session: print the opening turn and only the most recent N turns.
    #[arg(long, requires = "session", value_parser = clap::value_parser!(u16).range(1..=100))]
    pub(crate) recent: Option<u16>,
    /// With --session: start at the user turn with this message ID.
    #[arg(long, requires = "session", value_parser = clap::value_parser!(i64).range(1..))]
    pub(crate) from: Option<i64>,
    /// Number of preceding conversation messages to include.
    #[arg(short = 'B', long, default_value_t = 2, conflicts_with = "session")]
    pub(crate) before: usize,
    /// Number of following conversation messages to include.
    #[arg(short = 'A', long, default_value_t = 2, conflicts_with = "session")]
    pub(crate) after: usize,
    /// Number of messages to include on each side.
    #[arg(short = 'C', long, conflicts_with_all = ["before", "after", "session"])]
    pub(crate) context: Option<usize>,
    #[command(flatten)]
    pub(crate) habits: ReadOnlyHabits,
    /// Select a history profile; repeat to select several.
    #[arg(hide_short_help = true, long)]
    pub(crate) profile: Vec<String>,
    /// Total character budget for displayed message text.
    #[arg(long, default_value_t = 12000)]
    pub(crate) max_output_chars: usize,
    /// Show matching text without credential redaction.
    #[arg(hide_short_help = true, long)]
    pub(crate) no_redact: bool,
    /// Print complete selected text without output limits.
    #[arg(long, conflicts_with = "session")]
    pub(crate) full: bool,
}

#[derive(Args)]
pub(crate) struct SimilarArgs {
    /// Message ID from search or show.
    #[arg(value_parser = clap::value_parser!(i64).range(1..))]
    pub(crate) id: i64,
    /// Search these profiles instead of just the anchor's profile.
    #[arg(long)]
    pub(crate) profile: Vec<String>,
    /// Restrict results to a harness.
    #[arg(long, value_parser = SOURCE_KEYS)]
    pub(crate) source: Option<String>,
    /// Restrict results to a project.
    #[arg(long)]
    pub(crate) project: Option<String>,
    /// Restrict results to user or assistant messages.
    #[arg(long, value_parser = ["user", "assistant"])]
    pub(crate) role: Option<String>,
    /// Exclude every message in the anchor's session.
    #[arg(long)]
    pub(crate) other_sessions: bool,
    /// Maximum results to return.
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..=100))]
    pub(crate) limit: u16,
    /// Maximum results from one session.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u16).range(1..=100))]
    pub(crate) per_session: u16,
    /// Select the existing 768- or 256-dimensional Gemma 2 index.
    #[arg(long, default_value_t = 768)]
    pub(crate) dimensions: usize,
    #[command(flatten)]
    pub(crate) habits: ReadOnlyHabits,
}
#[derive(Subcommand)]
pub(crate) enum Profiles {
    /// Register an independent history root.
    Add {
        name: String,
        /// History format: codex, claude, or cursor.
        #[arg(long)]
        harness: Harness,
        /// History root directory for this independent profile.
        #[arg(long)]
        root: PathBuf,
        /// Optional account label.
        #[arg(long)]
        account: Option<String>,
        /// Optional provider/backend label.
        #[arg(long)]
        backend: Option<String>,
    },
    /// List the configured entries.
    List,
    /// Resume imports for this profile.
    Enable { name: String },
    /// Stop imports while retaining archived data.
    Disable { name: String },
    /// Change a profile root while preserving its identity.
    Relocate { name: String, root: PathBuf },
}
#[derive(Args, Default)]
pub(crate) struct ProfileSelection {
    /// Select a named history profile; repeat to select several.
    #[arg(long)]
    pub(crate) profile: Vec<String>,
    /// Select codex, claude, cursor, or all history sources.
    #[arg(long, value_parser = SOURCE_KEYS)]
    pub(crate) source: Option<String>,
    #[command(flatten)]
    pub(crate) habits: ReadOnlyHabits,
}
#[derive(Args, Default)]
pub(crate) struct ProjectSelection {
    /// Select a history profile; repeat to select several.
    #[arg(long, global = true)]
    pub(crate) profile: Vec<String>,
    /// Select codex, claude, cursor, or all history sources.
    #[arg(long,global=true,value_parser=SOURCE_KEYS)]
    pub(crate) source: Option<String>,
    /// Maximum projects to list (default: 100).
    #[arg(long, global = true)]
    pub(crate) limit: Option<usize>,
}

#[derive(Args, Default)]
pub(crate) struct Selection {
    /// Select a history profile; repeat to select several.
    #[arg(long)]
    pub(crate) profile: Vec<String>,
    /// Select codex, claude, cursor, or all history sources.
    #[arg(long, value_parser = SOURCE_KEYS)]
    pub(crate) source: Option<String>,
    /// Include records at or after this ISO date or timestamp; --after means the same.
    #[arg(long, alias = "after")]
    pub(crate) since: Option<String>,
    /// Include records at or before this ISO date or timestamp; --before means the same.
    #[arg(long, alias = "before")]
    pub(crate) until: Option<String>,
    /// IANA timezone for date filters and usage periods; local detects this system.
    #[arg(hide_short_help = true, long, default_value = "local")]
    pub(crate) timezone: String,
    /// Maximum results to return.
    #[arg(long, default_value = "100")]
    pub(crate) limit: Option<usize>,
}
#[derive(Args)]
pub(crate) struct SessionArgs {
    #[command(flatten)]
    pub(crate) selection: Selection,
    /// Include sessions created by subagents.
    #[arg(long)]
    pub(crate) include_subagents: bool,
    /// Only sessions of this kind, including hidden subagent and guardian sessions.
    #[arg(long, value_parser = SESSION_KINDS)]
    pub(crate) kind: Option<String>,
    /// Filter to an exact session working directory.
    #[arg(long)]
    pub(crate) cwd: Option<String>,
    /// Filter to working directories at or below this path.
    #[arg(long)]
    pub(crate) under: Option<String>,
    /// Filter by native project name or ID.
    #[arg(long)]
    pub(crate) project: Option<String>,
    /// List the oldest sessions first.
    #[arg(long)]
    pub(crate) oldest_first: bool,
    #[command(flatten)]
    pub(crate) habits: ReadOnlyHabits,
}
#[derive(Subcommand)]
pub(crate) enum Database {
    /// Reclaim unused space while retaining every stored table and row.
    Compact,
}
#[derive(Args)]
pub(crate) struct ProjectArgs {
    /// Include inactive projects and unused working-directory fallbacks.
    #[arg(long = "all", global = true)]
    pub(crate) include_inactive: bool,
    #[command(flatten)]
    pub(crate) selection: ProjectSelection,
    #[command(subcommand)]
    pub(crate) command: Option<ProjectCommand>,
}
#[derive(Subcommand)]
pub(crate) enum ProjectCommand {
    /// List discovered project roots and session counts.
    List,
    /// Refresh native project catalogs and their session assignments.
    Backfill,
}
#[derive(Args)]
pub(crate) struct ReprocessArgs {
    /// Select a history profile; repeat to select several.
    #[arg(long)]
    pub(crate) profile: Vec<String>,
    /// Select codex, claude, cursor, or all history sources.
    #[arg(long, value_parser = SOURCE_KEYS)]
    pub(crate) source: Option<String>,
    /// Select an archived source ID; repeat to select several.
    #[arg(long)]
    pub(crate) source_id: Vec<i64>,
    /// Report which archived sources would be reprocessed.
    #[arg(long)]
    pub(crate) dry_run: bool,
}
#[derive(Args)]
pub(crate) struct UsageArgs {
    #[command(flatten)]
    pub(crate) selection: Selection,
    /// Group usage by ISO week.
    #[arg(long)]
    pub(crate) weekly: bool,
    /// Split usage periods by model and reasoning effort.
    #[arg(long)]
    pub(crate) by_model: bool,
    #[command(flatten)]
    pub(crate) habits: ReadOnlyHabits,
}
#[derive(Args)]
pub(crate) struct Search {
    /// Words, code identifiers or a natural-language question.
    #[arg(allow_hyphen_values = true)]
    pub(crate) query: String,
    /// How the query is matched.
    #[arg(long, default_value = "words", value_parser = search_modes())]
    pub(crate) mode: Mode,
    /// What to search: messages, or the arguments of tool calls such as shell commands.
    /// Tool calls take --mode words or exact and ignore --role.
    #[arg(long = "in", default_value = "messages", value_parser = ["messages", "tools"])]
    pub(crate) scope: String,
    /// Rerank candidates using the configured local Qwen model or endpoint; needs --mode lexical, semantic or hybrid.
    #[arg(hide_short_help = true, long)]
    pub(crate) rerank: bool,
    /// Disable model reranking.
    #[arg(hide_short_help = true, long)]
    pub(crate) no_rerank: bool,
    /// Maximum candidates passed to the reranker.
    #[arg(hide_short_help = true, long,default_value_t=25,value_parser=clap::value_parser!(u16).range(1..=100))]
    pub(crate) rerank_depth: u16,
    /// Select a history profile; repeat to select several.
    #[arg(long)]
    pub(crate) profile: Vec<String>,
    /// Select codex, claude, cursor, or all history sources.
    #[arg(long, value_parser = SOURCE_KEYS)]
    pub(crate) source: Option<String>,
    /// Include records at or after this ISO date or timestamp; --after means the same.
    #[arg(long, alias = "after")]
    pub(crate) since: Option<String>,
    /// Include records at or before this ISO date or timestamp; --before means the same.
    #[arg(long, alias = "before")]
    pub(crate) until: Option<String>,
    /// IANA timezone for date filters and usage periods; local detects this system.
    #[arg(hide_short_help = true, long, default_value = "local")]
    pub(crate) timezone: String,
    /// Whose messages to search: the user's, the assistant's final answers, or all.
    /// With all, a ranked mode shows the answer to a matching question.
    #[arg(long, default_value = "user", value_parser = ["user", "assistant", "all"])]
    pub(crate) role: String,
    /// Include this native thread ID; repeat to select several.
    #[arg(hide_short_help = true, long)]
    pub(crate) thread: Vec<String>,
    /// Filter to an exact session working directory.
    #[arg(hide_short_help = true, long)]
    pub(crate) cwd: Option<String>,
    /// Filter to working directories at or below this path.
    #[arg(long)]
    pub(crate) under: Option<String>,
    /// Filter by native project name or ID.
    #[arg(long)]
    pub(crate) project: Option<String>,
    /// Exclude subagent sessions.
    #[arg(hide_short_help = true, long, overrides_with = "_include_subagents")]
    pub(crate) no_subagents: bool,
    /// Python's explicit default; overrides an earlier --no-subagents.
    #[arg(
        long = "include-subagents",
        hide = true,
        overrides_with = "no_subagents"
    )]
    pub(crate) _include_subagents: bool,
    /// Include agent coordination and protocol messages.
    #[arg(hide_short_help = true, long)]
    pub(crate) include_agent_protocol: bool,
    /// Only sessions of this kind; subagent and guardian include their protocol messages. Guardian prompts are searched by their reviewed action, not the quoted transcript.
    #[arg(long, value_parser = SESSION_KINDS, conflicts_with = "no_subagents")]
    pub(crate) kind: Option<String>,
    /// Maximum results to return.
    #[arg(long,alias="top",default_value_t=8,value_parser=clap::value_parser!(u16).range(1..=100))]
    pub(crate) limit: u16,
    /// Maximum search results from one session.
    #[arg(long,default_value_t=2,value_parser=clap::value_parser!(u16).range(1..=100))]
    pub(crate) per_session: u16,
    /// Maximum characters in a result passage.
    #[arg(hide_short_help = true, long, default_value_t = 1000)]
    pub(crate) snippet_chars: usize,
    /// Total character budget for displayed message text.
    #[arg(long, default_value_t = 8000)]
    pub(crate) max_output_chars: usize,
    /// Print complete selected text without output limits.
    #[arg(long)]
    pub(crate) full: bool,
    /// Show matching text without credential redaction.
    #[arg(hide_short_help = true, long)]
    pub(crate) no_redact: bool,
    /// Include retrieval scores, model coverage and timing details.
    #[arg(hide_short_help = true, long)]
    pub(crate) explain: bool,
    /// Search the existing archive without importing live changes.
    #[arg(long, conflicts_with = "force_refresh", overrides_with = "_refresh")]
    pub(crate) no_refresh: bool,
    /// Python's explicit default; overrides an earlier --no-refresh.
    #[arg(long = "refresh", hide = true, overrides_with = "no_refresh")]
    pub(crate) _refresh: bool,
    /// Refresh now, bypassing the change check and failure backoff.
    #[arg(hide_short_help = true, long)]
    pub(crate) force_refresh: bool,
    /// Skip automatic background embedding work.
    #[arg(long, overrides_with = "_background")]
    pub(crate) no_background: bool,
    /// Python's explicit default; overrides an earlier --no-background.
    #[arg(long = "background", hide = true, overrides_with = "no_background")]
    pub(crate) _background: bool,
    #[command(flatten)]
    pub(crate) inference: Inference,
}
#[derive(Args, Clone)]
pub(crate) struct Inference {
    /// Allow automatic background work to use the configured remote endpoint.
    #[arg(hide_short_help = true, long, env = "LLM_HISTORY_RS_BACKGROUND_REMOTE")]
    pub(crate) background_remote: bool,
    /// Compatible embedding server URL; otherwise use cached local Gemma.
    #[arg(
        hide_short_help = true,
        long,
        env = "LLM_HISTORY_RS_EMBEDDING_ENDPOINT"
    )]
    pub(crate) endpoint: Option<String>,
    /// Model alias served by the embedding endpoint.
    #[arg(hide_short_help = true, long)]
    pub(crate) model_name: Option<String>,
    /// Read the embedding endpoint API key from this file.
    #[arg(hide_short_help = true, long)]
    pub(crate) api_key_file: Option<PathBuf>,
    /// Compatible reranking server URL; otherwise use cached local Qwen.
    #[arg(hide_short_help = true, long, env = "LLM_HISTORY_RS_RERANKER_ENDPOINT")]
    pub(crate) rerank_endpoint: Option<String>,
    /// Model alias served by the reranking endpoint.
    #[arg(hide_short_help = true, long)]
    pub(crate) rerank_model_name: Option<String>,
    /// Read the reranking endpoint API key from this file.
    #[arg(hide_short_help = true, long)]
    pub(crate) rerank_api_key_file: Option<PathBuf>,
    /// Gemma embedding dimensions: 768 or 256.
    #[arg(hide_short_help = true, long, default_value_t = 768)]
    pub(crate) dimensions: usize,
    /// Maximum inference request/startup time in seconds.
    #[arg(hide_short_help = true, long, default_value_t = 120)]
    pub(crate) timeout_seconds: u64,
}
impl Default for Inference {
    fn default() -> Self {
        Self {
            background_remote: false,
            endpoint: None,
            model_name: None,
            api_key_file: None,
            rerank_endpoint: None,
            rerank_model_name: None,
            rerank_api_key_file: None,
            dimensions: 768,
            timeout_seconds: 120,
        }
    }
}
#[derive(Subcommand)]
pub(crate) enum Raw {
    /// Check the selected archive against its stored record boundaries.
    Verify {
        #[command(flatten)]
        selection: RawSelection,
        /// Compare the archived byte prefix with the live source.
        #[arg(long)]
        against_live: bool,
        #[arg(long, short)]
        verbose: bool,
    },
    /// List retained source generations.
    Sources(RawSelection),
    /// Show raw source counts, sizes and compression.
    Stats(RawSelection),
    /// Reconstruct original source bytes from the archive.
    Export {
        #[command(flatten)]
        selection: RawSelection,
        #[arg(
            long,
            short,
            required_unless_present = "stdout",
            conflicts_with = "stdout"
        )]
        output: Option<PathBuf>,
        /// Write the reconstructed source bytes to stdout.
        #[arg(long)]
        stdout: bool,
        /// Replace an existing output, or rebuild an index when requested.
        #[arg(long)]
        force: bool,
    },
}
#[derive(Args)]
pub(crate) struct RawSelection {
    /// Source ID, native thread ID, or exact source path.
    pub(crate) selector: Option<String>,
    /// Select a history profile; repeat to select several.
    #[arg(long)]
    pub(crate) profile: Vec<String>,
    /// Select codex, claude, cursor, or all history sources.
    #[arg(long, value_parser = SOURCE_KEYS)]
    pub(crate) source: Option<String>,
    /// Select one retained source generation.
    #[arg(long,value_parser=clap::value_parser!(i64).range(1..))]
    pub(crate) generation: Option<i64>,
}
impl RawSelection {
    pub(crate) fn filter(self, latest: bool) -> llm_history_types::archive::RawFilter {
        llm_history_types::archive::RawFilter {
            profiles: self.profile,
            source: self.source,
            selector: self.selector,
            generation: self.generation,
            latest,
        }
    }
}
#[derive(Subcommand)]
pub(crate) enum Archive {
    /// Save a checkpointed database snapshot.
    Create {
        output: Option<PathBuf>,
        /// Optional label included in the generated archive filename.
        #[arg(long, conflicts_with = "output")]
        label: Option<String>,
    },
    /// Restore a saved database snapshot.
    Restore {
        archive: PathBuf,
        /// Confirm replacing the active database; retain its previous checkpoint.
        #[arg(long)]
        yes: bool,
    },
    /// Check the selected archive against its stored record boundaries.
    Verify { archive: PathBuf },
    /// List saved database snapshots.
    List {
        /// Directory containing database archives.
        #[arg(long)]
        directory: Option<PathBuf>,
    },
}
#[derive(Subcommand)]
pub(crate) enum Models {
    /// List the configured entries.
    List,
    /// Download both local models and their tokenizer into the Hugging Face cache.
    Fetch {
        /// Explicitly allow downloading the pinned models into the Hugging Face cache.
        #[arg(long)]
        download_model: bool,
    },
    /// Recheck the cached model files on request.
    Verify,
}
#[derive(Subcommand)]
pub(crate) enum Semantic {
    /// Build or refresh the lexical index.
    Index {
        /// Rebuild an existing lexical index.
        #[arg(long)]
        force: bool,
    },
    /// Search through the shared pipeline; defaults to hybrid retrieval.
    #[command(mut_arg("mode", |arg| arg.default_value("hybrid")))]
    Search(Box<Search>),
    /// Show embedding coverage for the selected profiles.
    Status {
        #[command(flatten)]
        inference: Inference,
        /// Select a history profile; repeat to select several.
        #[arg(long)]
        profile: Vec<String>,
    },
    /// Start a bounded detached embedding worker, or wait with --foreground.
    Backfill(Backfill),
    /// Show the last worker state and pending embedding coverage.
    BackfillStatus {
        #[command(flatten)]
        inference: Inference,
    },
}
#[derive(Args, Clone)]
pub(crate) struct Backfill {
    /// Wait for the embedding worker in this process.
    #[arg(long)]
    pub(crate) foreground: bool,
    /// Total worker time budget, including database waits.
    #[arg(long, default_value_t = 300.0)]
    pub(crate) seconds: f64,
    /// Maximum documents processed in this worker run.
    #[arg(long, default_value_t = 10000)]
    pub(crate) max_documents: usize,
    /// Maximum input tokens processed in this worker run.
    #[arg(long, default_value_t = 2000000)]
    pub(crate) max_tokens: usize,
    /// Maximum queued inference data in MiB.
    #[arg(long, default_value_t = 64)]
    pub(crate) queue_mib: u64,
    /// Maximum queued inference items.
    #[arg(long, default_value_t = 1024)]
    pub(crate) queue_items: usize,
    /// Skip documents exceeding this character count.
    #[arg(long, default_value_t = 1_000_000)]
    pub(crate) max_document_chars: usize,
    /// Maximum pending documents read at once.
    #[arg(long, default_value_t = 64)]
    pub(crate) read_batch: usize,
    /// Select codex, claude, cursor, or all history sources.
    #[arg(long, value_parser = SOURCE_KEYS)]
    pub(crate) source: Option<String>,
    /// Retry documents previously marked as embedding failures.
    #[arg(long)]
    pub(crate) retry_failures: bool,
    /// Select a history profile; repeat to select several.
    #[arg(long)]
    pub(crate) profile: Vec<String>,
    #[command(flatten)]
    pub(crate) inference: Inference,
}

#[derive(Debug, clap::Args)]
pub(crate) struct SqlArgs {
    /// DuckDB query; discover available surfaces through history_help.
    pub(crate) query: String,
    /// Maximum rows to display. Use SQL LIMIT to limit the query itself.
    #[arg(long, alias = "max-rows", default_value_t = 50)]
    pub(crate) limit: usize,
    /// Maximum UTF-8 bytes per SQL header or text cell.
    #[arg(
        hide_short_help = true,
        long,
        alias = "max-cell-chars",
        default_value_t = 2000
    )]
    pub(crate) max_cell_bytes: usize,
    /// Maximum serialized SQL output bytes.
    #[arg(long, default_value_t = 16000)]
    pub(crate) max_output_bytes: usize,
    /// Interrupt DuckDB execution after this many seconds, including with --full.
    #[arg(long, default_value_t = llm_history_types::storage::DEFAULT_SQL_TIMEOUT_SECONDS, value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) timeout_seconds: u64,
    /// Print complete selected text without output limits.
    #[arg(long)]
    pub(crate) full: bool,
    /// Show matching text without credential redaction.
    #[arg(hide_short_help = true, long, overrides_with = "_redact")]
    pub(crate) no_redact: bool,
    /// Python's explicit default; overrides an earlier --no-redact.
    #[arg(long = "redact", hide = true, overrides_with = "no_redact")]
    _redact: bool,
    #[command(flatten)]
    pub(crate) habits: ReadOnlyHabits,
}

#[cfg(test)]
mod python_flag_tests {
    use super::*;
    #[test]
    fn every_mode_is_chosen_with_the_mode_flag_on_either_search_command() {
        let mode = |command: &[&str], flags: &[&str]| {
            let line = [&["llm-history"], command, &["q"], flags].concat();
            match App::try_parse_from(line).map(|app| app.command) {
                Ok(Commands::Search(args) | Commands::Semantic(Semantic::Search(args))) => {
                    Some(args.mode)
                }
                _ => None,
            }
        };
        let (plain, semantic): (&[&str], &[&str]) = (&["search"], &["semantic", "search"]);
        assert_eq!(mode(plain, &[]), Some(Mode::Words));
        assert_eq!(mode(semantic, &[]), Some(Mode::Hybrid));
        for command in [plain, semantic] {
            for (name, expected) in [
                ("words", Mode::Words),
                ("lexical", Mode::Lexical),
                ("exact", Mode::Exact),
                ("semantic", Mode::Semantic),
                ("hybrid", Mode::Hybrid),
                ("auto", Mode::Auto),
            ] {
                assert_eq!(mode(command, &["--mode", name]), Some(expected));
            }
            assert_eq!(mode(command, &["--mode", "exakt"]), None);
            assert_eq!(mode(command, &["--exact"]), None);
        }
    }
    #[test]
    fn kind_is_validated_and_cannot_exclude_what_it_names() {
        let parse = |flags: &[&str]| {
            App::try_parse_from(["llm-history", "search", "q"].iter().chain(flags)).is_ok()
        };
        assert!(parse(&["--kind", "guardian"]));
        assert!(!parse(&["--kind", "reviewer"]));
        assert!(!parse(&["--kind", "guardian", "--no-subagents"]));
    }
    #[test]
    fn explicit_python_defaults_override_earlier_negations() {
        let app = App::try_parse_from([
            "llm-history",
            "search",
            "q",
            "--no-subagents",
            "--include-subagents",
            "--no-refresh",
            "--refresh",
            "--no-background",
            "--background",
        ])
        .unwrap();
        let Commands::Search(args) = app.command else {
            panic!("search")
        };
        assert!(!args.no_subagents && !args.no_refresh && !args.no_background);
        let app = App::try_parse_from([
            "llm-history",
            "search",
            "q",
            "--include-subagents",
            "--no-subagents",
        ])
        .unwrap();
        let Commands::Search(args) = app.command else {
            panic!("search")
        };
        assert!(args.no_subagents);
        let app =
            App::try_parse_from(["llm-history", "sql", "select 1", "--no-redact", "--redact"])
                .unwrap();
        assert!(matches!(
            app.command,
            Commands::Sql(SqlArgs {
                no_redact: false,
                ..
            })
        ));
        let app = App::try_parse_from(["llm-history", "update", "--no-background", "--background"])
            .unwrap();
        assert!(matches!(
            app.command,
            Commands::Update {
                no_background: false,
                ..
            }
        ));
    }
}

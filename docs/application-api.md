# Application interface

`llm-history-app` is the programmatic entry point. `llm-history-cli` owns argument
parsing, environment/home defaults, presentation, exit codes and detached process
launching. The application crate owns history operations and their shared lifecycle.
Shared values have canonical paths in `llm-history-types`. Session, project and
usage results are owned by `llm_history_types::query` and returned directly by
storage. Inference interfaces are imported from models and worker configuration
from search.

```rust,no_run
use llm_history_app::{
    History,
    api::{ingestion::UpdateRequest, query::MessageQuery},
};
use llm_history_types::query::{Filter, SearchRequest};

fn example() -> anyhow::Result<()> {
    let history = History::new("history.duckdb");
    history.create()?;
    // Register explicit profiles before updating a new library store.
    let outcome = history.update(&UpdateRequest::default())?;
    let sessions = history.sessions(&Filter::default(), 30, false)?;
    let page = history.messages(&MessageQuery::default())?;
    let hits = history.search(
        &SearchRequest { query: "repair history".into(), ..Default::default() },
        None,
        None,
    )?;
    Ok(())
}
```

Application-owned requests/results are defined in `api::query`, `api::ingestion`,
`api::sql` and `api::search`. Imports name those modules directly. The application
crate forwards no foreign symbols. Archive-file verification, listing and restore
have their sole API in `llm_history_store::archives`; database workflows use
`History`. CLI source checks allow only those three archive-file functions from
storage.

Store connections are private. Index readiness/receipts, schema readiness and
refresh timestamps use typed storage operations. The engine connection and
transaction capabilities are restricted to the storage crate family.

## Handle and effects

Construct `History` with an explicit database path and `HistoryOptions`. Construction
does no I/O. The handle owns configuration, never a database connection. Reads
neither initialize nor upgrade a store, refresh sources, launch workers nor
download models. `initialize`, `update`, `refresh`, `reprocess`, indexing and
archive maintenance are explicit operations.

Live transcript and catalog preparation run between database connections. A
separate ingestion lock serializes source changes, profile changes and restore
across those intervals. Readers can use committed history between publication
batches; a concurrent automatic refresh can skip active ingestion. Expected
checkpoints still guard publication. Replay reads the retained archive through
a shared connection and acquires a writer for publication.

Default profile candidates are explicit inputs. Only initialization/update/refresh
consider them, and only when no profiles exist. The CLI supplies its existing Codex
and Claude environment/home defaults. Library callers can leave the list empty.

## Initial public contract

- Profile registration, enable/disable and relocation preserve profile identity.
- Session/project/usage listings have typed serializable responses. Filters, ordering
  and limits retain current behavior. Session date filters use session start; message
  date filters use event time. Existing inclusive date bounds remain inclusive.
- `messages` and `context` return complete normalized message text. Context is ordered
  within its session and remains available for explicitly addressed protocol messages.
  Message queries page in opaque-ID order with an exclusive `after` cursor and a
  limit of 1–1000. Context takes a short message number and retains the existing
  maximum of 100 messages on each side.
- Search returns `MessageExcerpt`, a separate type whose text may be redacted,
  shortened or selected from an answer. Existing ranked answer selection, exact hit
  selection, ordering, guardian handling and limits are preserved. Retrieve complete
  text through `context`. Exact search alone reports a total match count. Set
  `SearchRequest::explain` to count eligible documents and embedding coverage.
- IDs are opaque, scoped to their database. Native IDs additionally require profile
  and harness scope. Retained occurrence IDs survive append, reopen and replay.
  Independent imports need not allocate the same IDs. Derived conversation memberships
  can change as parent evidence changes.
- `similar` takes a short message number and scores current, stored Gemma 2
  document vectors. The anchor's history profile is the default scope. The
  returned excerpts identify the actual scored messages; no inference or refresh runs.
- Update/replay receipts can report successful work alongside per-source errors.
  Replay errors include the failed stage and a bounded, redacted cause; a failed
  source retains its previous projection. Callers inspect receipts; the CLI
  retains its existing nonzero partial-failure exit.
- Bounded read-only SQL and the documented `history_help` views/macros retain their
  names, selected columns and semantics. `SqlRequest::timeout_seconds` defaults
  to 15 and interrupts DuckDB independently of the output limits. A row cap bounds
  the response; use SQL `LIMIT` to bound the selected rows when appropriate.
  SQL rows and diagnostics deliberately use JSON values. Internal tables, staging SQL and parser checkpoints are private contracts.
- Raw generation export streams exact captured bytes. For Cursor SQLite, the
  capture is a JSONL stream of retained rows, and live file byte comparison
  does not apply. Verification remains explicit.
- Search/inference accept the existing engine traits, supporting deterministic fakes.
  Bounded backfill preserves cancellation, staging/resume and stale-publication checks.
  Restart loading enforces the configured queue byte/item limits while decoding and
  checks cancellation/deadlines during reads. A larger retained queue returns an error
  before inference or publication and keeps its staging files intact; resume with
  sufficient `queue_bytes`/`queue_items` (`--queue-mib`/`--queue-items` in the CLI).
  Foreground search embeds the query before opening its retrieval connection and
  releases that connection before reranking. Background document inference also
  runs outside a database connection. Foreground and detached backfill use the
  caller's database resource settings and the worker's own cancellation and
  lock-wait budget.

Codex thread metadata acquisition is limited to 100,000 rows and 32 MiB across
both native database locations, with a ten-second deadline and a SQLite value
length limit. Acquisition finishes before the destination writer opens.
Failed metadata acquisition retains the previous metadata and fingerprint and
appears in the update receipt alongside already committed transcript captures.

Internal Rust interfaces can change in this unreleased application. Shared
identity, query, configuration and receipt values are imported directly from
`llm-history-types`. Application-owned requests and responses describe workflows.
Existing CLI JSON aliases and formatting stay in the CLI.

## Acceptance fixtures

`fixtures/contracts` contains public synthetic native inputs, a versioned JSON scenario
manifest and reviewed expected observations. Tests populate isolated stores through
public application operations and check public responses, documented SQL, identities
across lifecycle steps and exact raw exports. The same scenario is exercised through
the CLI. Fixture versioning is independent of database/schema/parser versions.

The pre-extraction executable at commit `7bd5ed1` is the temporary compatibility
oracle. Compare equivalent isolated CLI runs, normalizing only declared temporary
paths, generated profile/store UUIDs and diagnostic execution timestamps/durations.
Content, native identifiers, event times, order, exit codes and retained IDs remain
significant. Known behavior changes require their own reviewed changes.

The optional CLI differential test currently also compares numeric entity IDs across
identical fresh runs. Permanent application tests establish stability within a store
and compare independent identities by their profile/native references.

Adapter extraction, canonical-model expansion, remote model-contract corrections
and privatizing the store connection follow this initial application boundary.

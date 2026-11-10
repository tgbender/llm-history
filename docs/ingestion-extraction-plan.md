# Ingestion workflow

**Implemented snapshot: 2026-11-09.** The Codex and Claude workflows now
prepare source data before starting a destination write transaction. This
snapshot describes the current ownership and the remaining design questions.
See the [harness format references](harness-history/README.md) and
[storage assessment](ingestion-storage.md) for native evidence and candidate
storage models.

| Owner                                  | Responsibility                                                                                                                            |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `app/src/ingestion/`                   | Sequence update, repair, catalog refresh and retained replay. Supply stored inputs, bound preparation files and report outcomes.          |
| `ingest::source` and `ingest::prepare` | Discover transcript files and assign stable keys, interpret checkpoints, acquire records and request adapter-specific resume state.       |
| Per-harness ingest modules             | Interpret native records and catalogs, select reusable body leaves and maintain decoder state.                                            |
| `core` capture modules                 | Bounded JSONL/zstd input, original decoded offsets and xxh64 continuity checks.                                                           |
| `types::ingestion`                     | Checkpoints, prepared records and catalog values shared between preparation and publication.                                              |
| `store/history`                        | Supply checkpoints and retained records, allocate IDs, reconstruct archived bytes and publish captures, facts and checkpoints atomically. |
| `store/harnesses`                      | Write native metadata in the shared history transaction.                                                                                  |

An update asks ingest for a finite inventory of each profile's candidate
files and stable keys. The planner uses unchanged size and mtime to skip reads,
then xxh64 windows (or the saved
prefix hash for older checkpoints) to decide whether a changed file can resume.
Each source is opened with a fixed physical cutoff. Only complete records advance
the checkpoint. An append beyond the cutoff is left for another invocation.
Identity or continuity failures leave the last committed state available; the
app does not rediscover or reacquire a source to chase an active writer.
When continuity checks allow a Claude append, preparation requests the source's
previously counted usage keys through the app.

The app spools prepared records to bounded private files without holding a database
connection. A separate ingestion lock serializes inventory and publication with
other imports, profile changes and restores. Store opens a writer for each
publication batch and checks the exact expected checkpoint. A failed batch may be retried once
per source using the same prepared input. A rewrite switches the active
generation atomically; earlier retained bytes remain exportable. Catalog updates
carry their own revision and can publish independently of transcript changes.

Replay uses the retained native documents and saved content identities. The app
runs the decoder while reading retained records with a shared connection, then
opens a writer for the replacement facts and continuation. The receipt identifies
failed sources, the stage and a bounded cause. The decoder revision is independent of the database schema: this pre-alpha
application uses schema 1 and earlier development databases are converted
offline. One-off Python conversions are local SQL outside the runtime.

Public behavior is exercised through `History` and the CLI. Storage tests cover
atomicity, stale checkpoints and byte-exact reconstruction; capture and decoder
tests cover source bounds and wire formats. Shared synthetic fixtures supply
inputs without depending on a user's history.

## Follow-up design

The app still holds the destination writer handle while preparing input.
Releasing it would require checking destination identity on reopen and scoping
absence to the inventory that was actually observed. Preparation has finite
per-source and batch limits. Resumable staging for very large rewrites and a
shared run deadline with explicit deferred outcomes are possible follow-ups.

Codex catalog hints can help select candidate files; timestamps and projection
cursors can lag the transcript, so they cannot replace the file cutoff. Future
prefix resolution needs to bind the precise retained parent generation and
boundary, with visited and byte budgets. Mutable SQLite sources need bounded
snapshots, settled admission rules and explicit coverage. Those changes can use
the same publication boundary once their native contracts have fixture coverage.

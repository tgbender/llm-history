# DuckDB batching plan

**Started 2026-11-09.** Live publication, epoch projection, retained reads,
catalog writes, and clean-refresh gating are implemented. The original
release baseline held 2,400 synthetic records constant: 12 to 120 files
raised import time from 0.68 to 5.24 seconds and replay from 0.35 to 1.90
seconds. CI measures SQL work and preserved behavior; release timings are
recorded separately.

Keep one serialized publication path. Use bounded batches of source keys and
rows, with typed lists for small inputs and the existing Appender/staging
pattern for large ones. Reuse the selected set across statements. Push scope
and limits before joins that hydrate wide rows, and use semi and anti joins
for membership and absence. Keep ordered decoding, byte-exact native history,
stable existing IDs, profile isolation, source cutoffs and atomic checkpoint
advancement. Leave schema at 1; any development database conversion remains
offline.

## 0. Record the baseline and add regression checks

- Preserve a release executable from the current worktree before editing.
  Benchmark initial import, append, no-op refresh, dry and full replay, and
  raw verification on temporary stores. Vary file count at fixed record and
  byte volume; also use a single long session that spans multiple flushes,
  includes compactions and links, and gets a small append.
- In storage tests, count expensive statements on a synthetic connection.
  The pinned DuckDB supports in-memory `QueryLog` via `enable_logging` and
  `duckdb_logs()`. Measure CAS deduplication, projection, archive reads and
  reconciliation separately. Use `EXPLAIN ANALYZE` to inspect rows and work
  in joins and aggregates. Keep logging off in normal operation.
- Gate on statement counts and behavior, rather than elapsed time in CI.
  Work should scale with bounded batches and touched scopes. Report release
  wall time, peak memory and writer lock duration separately. Keep
  `mise run check` and the public synthetic contracts as behavior gates.
  Add a focused AST check that surfaces new data-dependent SQL calls in loops
  for review. Query counts catch calls made through helpers.

## 1. Live publication batches (implemented)

[Publication](../crates/store/history/src/publication.rs) stages the bounded
plan, reads and validates exact checkpoints and current generations in one
query, reserves IDs in sets, and retires replaced projections together.
[One buffer](../crates/store/history/src/batch.rs) stages archive and fact
rows across files and flushes by row and byte limits. Replacements and
replays match existing fact IDs against exact staged keys within each flush;
ordinary appends do not load historical IDs.

[Session updates](../crates/store/history/src/session_batch.rs) are resolved
by profile and native ID, preserving ordered metadata precedence.
Generations, captures, checkpoints, identical repairs and final source
ownership are published in sets. The application can isolate a failed
group using the same prepared bytes, with no live reread or retry loop.

The [publication tests](../crates/store/history/src/tests/publication.rs)
hold the native record count constant across 12 and 48 files and count CAS,
session, projection and finalization SQL on a synthetic connection. They
also verify rollback after an earlier flush and stable sequence ranges
and raw IDs through mixed append, rewrite and repair. In three release
runs of the 2,400-record fixture, median import fell from 0.685 to 0.244
seconds for 12 files and from 5.286 to 0.586 seconds for 120 files.

## 2. Carry the epoch cursor across projection flushes (implemented)

[Append publication](../crates/store/history/src/append_epoch_seeds.sql)
loads the latest committed epoch endpoint for each affected source and
session in one query. It fetches only the selected event IDs to determine
whether the last event closed its epoch. That endpoint remains valid when
raw-only lines follow the last projected event.

[Projection](../crates/store/history/src/canonical_batch.sql) carries the
next epoch across bounded flushes in transaction-local state. New
captures, replacements, replays, and full ownership corrections start
from the source's initial epoch. The publication regression checks one
seed for multiple flushes, compactions on batch boundaries, distinct
physical files in one session, stable IDs on replay, and a rewrite
followed by an append.

The once-per-publication finalizer still recomputes epoch endpoints,
session bounds, and native relationships for affected sources or sessions.
A follow-up can update only touched epochs and endpoints and resolve
retained children when their native parents arrive.

## 3. Read retained history in bounded sets (implemented)

[Replay](../crates/store/history/src/reprocess.rs) selects checkpoints and
generations together, including requested source IDs. Replay and
[raw verification](../crates/store/history/src/raw.rs) use the same
[archive reader](../crates/store/history/src/archive_data.rs). It issues
two ordered BLOB queries per bounded group, with ordered ordinal ranges
for oversized generations. The limits account for observation count and
decoded byte size. A failed group can isolate a corrupt generation once.
Replay checkpoint validation and updates use one staged set. Raw stats
group observations by payload multiplicity before aggregating byte sizes.

Capture and replay also carry observation IDs directly into fact staging.
Projection can link an event to its native record without looking the
observation back up on every flush. Tests check query counts, ordered
oversized reads, exact export, and stable replay IDs.

## 4. Clean refresh and metadata in sets (implemented)

- File discovery indexes exact checkpoint keys and relocation basenames.
  After bounded source opens validate the cutoffs, Claude usage-key reads
  and changed source locations are handled in sets. Inventory reconciliation
  updates missing flags without loading full decoder state again.
- Publication records dirty project and conversation profiles atomically.
  Clean scopes skip reconciliation; dirty work survives interruption.
  Graph rebuilds only the marked profiles, with full rebuilding available
  to repair. The graph compresses parent paths while grouping chains.
- Catalogs stage project rows, roots and explicit assignments. Changed
  assignment sets use anti joins. Reconciliation resolves explicit matches
  in one query, uses a path-prefix index for root precedence, deduplicates
  fallback CWDs, and applies mutations together. Project list selects the
  page before fetching roots and counts.
- The remaining metadata work is to load decoder continuation only for
  selected files, scope native-thread metadata synchronization to changed
  keys, and measure whether dirty conversation components need a narrower
  scope than a profile.

## 5. Finish query and worker findings

| Finding                            | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | Verification                                                                                                                                                                                 |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Session listings                   | Apply typed date and other eligibility filters before Top N and hydrate relations and effort for selected IDs. Project pages already hydrate roots and counts in sets.                                                                                                                                                                                                                                                                                                    | Compare ordering, filters and counts with public fixtures.                                                                                                                                   |
| Search answer pairing              | Forward ASOF joins now find the next user and first eligible answer within each physical source/session. Ordinary exact hits skip answer lookups; guardian verdicts retain them.                                                                                                                                                                                                                                                                                          | Use long turns, interleaved tools, hidden messages, ordinal ties and guardian hits. Check intermediate row counts and result parity.                                                         |
| Context, lineage and tool activity | Read bounded ranges around the message anchor; seed recursion from the requested root and hydrate narrow edges after selection. Require nonempty tool call IDs for pairing and reuse the result's sequence. Remove redundant joins in message/timeline views where the same ID is already available.                                                                                                                                                                      | Check cross-profile isolation, context order, branch coverage and missing tool IDs. Inspect plans for bounded intermediate sets.                                                             |
| Usage and status                   | Usage selects profiles before deduplication, applies time bounds after winner selection, and aggregates and limits periods in SQL. Status counts and time bounds remain candidates for shared scoped relations.                                                                                                                                                                                                                                                           | Compare usage dedup, timezone/weekly buckets and the legacy/canonical counts. Check scans per relation.                                                                                      |
| SQL output and FTS                 | Bound SQL result materialization with a safe engine-side limit where compatible and a tested fallible streaming cursor for other statements. Inspect FTS candidate plans; if they scan broad eligible rows before limiting, compare a set-returning scoring query with the same result order and semantics.                                                                                                                                                               | Check output/truncation metadata, error paths and exact lexical ranks. Measure plans with selective and broad inputs before changing FTS.                                                    |
| Embedding worker and index         | Keep bulk vector publication and native `FLOAT[]`. Production and tests share bulk publication; the obsolete per-vector writer is gone. Resolve scope once per read cycle, fetch narrow candidate rows and defer oversized counts until final drain. Check staged work before the pending probe; avoid redundant profile writes and double builds on fresh forced indexing. Prefer the pinned tokenizer and profile remote or queue overhead before changing concurrency. | Check batched writes, stale hash and identity checks, restart/cancellation, writer lock lifetime, model scope and index readiness. Profile pending anti joins with a mature embedding table. |

## 6. Remaining archive and finalization cost

[CAS publication](../crates/store/history/src/archive_publish.sql) selects
new document hashes in one anti join per bounded flush. It reuses that set
to insert only referenced new bodies, parts, and documents. The temporary
plan holds keys; compressed bytes stay in staging. A bounded cache of
document hashes already published within the transaction skips subsequent
probes for repeated documents. Compare alternative
layouts on lookup cost, file growth, and checkpoint writes together; an ART
index on the full hash improved lookup time in a synthetic probe but
substantially increased file size and checkpoint time.

Finish scoped epoch and relationship updates so small appends avoid revisiting
unaffected history. Keep one-off conversions in ignored local SQL. Measure
writer lock hold time separately from the ingestion lock. Live preparation
releases the database writer; the ingestion lock serializes competing source
changes and restore, and publication still revalidates expected checkpoints.

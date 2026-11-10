# Storage crates

**Implemented snapshot: 2026-11-09.** This directory groups three workspace
packages. Each owns actual implementation; the directory has no facade package.

| Package                       | Directory    | Owner                                                                                                             |
| ----------------------------- | ------------ | ----------------------------------------------------------------------------------------------------------------- |
| `llm-history-store-engine`    | `engine/`    | Configured DuckDB connections, stable file locks, transaction lifetime, bundled FTS/ICU and SQL hash registration |
| `llm-history-store`           | `history/`   | Shared history tables, queries, projection/checkpoint publication, archive operations and schema initialization   |
| `llm-history-store-harnesses` | `harnesses/` | Native Codex thread metadata and retained project-catalog snapshots/assignments                                   |

```text
history -> engine
        -> harnesses -> engine
```

All storage packages use the same pinned DuckDB binding. Only engine directly
owns the sys crate/native build. Shared logical/native observation values live in
`llm-history-types`. Ingestion reads source SQLite/JSON metadata and decodes it;
harness storage receives captured values and never opens source databases.

## Implemented modules

```text
engine/
  build.rs
  src/{lib.rs,extensions.cpp,hashing.rs}
history/src/
  lib.rs
  metadata.rs
  schema/{mod.rs,sql/}
  archives.rs
  batch.rs
  catalogs.rs
  ... existing query/projection/replay modules
  tests/
harnesses/src/
  lib.rs
  schema.rs
  catalog.rs
  catalog.sql
  codex/{mod.rs,threads.rs,schema.sql}
```

History retains its package name and actual Store implementation in `history/`.
Consumers name the defining crate/module directly. The connection is private;
application index/schema/refresh work uses typed storage capabilities. Physical
SQL fault-injection tests live inside history's test modules. Public application
and CLI acceptance suites continue to exercise their own interfaces.

## Transactions and schema activation

Engine owns the connection and stable lock. `Database::begin` creates a borrowed
`WriteTransaction`; its underlying driver transaction rolls back when dropped.
Native writers accept `&WriteTransaction`, and history owns the final commit.
Catalog snapshots, assignments and thread rows therefore participate in the same
transaction as shared project/session changes and metadata checkpoints. Tests
cover a late SQL failure rolling back both owners and a committed reopen.

Engine registers `llm_history_blake3(BLOB) → BLOB` and
`llm_history_xxh64(BLOB) → UBIGINT` on application connections, including
read-only ones. They reuse the pinned Rust hash libraries, preserve NULL,
and process DuckDB chunks. They hash exactly the supplied bytes. Normal
capture, replay and archive verification retain their current hash paths.

The history schema runner applies the shared SQL assets and the harness schema
components before derived views. The pre-alpha database uses schema 1: native
observations, capture ranges and shared body storage are part of the initial DDL.
Earlier development databases are converted offline. Decoder revision is tracked
separately so the app can rebuild projections from retained records. See the [retained capture model](../../docs/schema/README.md)
for archive encoding and replay behavior.

Compaction closes the engine connection while retaining its stable lock across
file replacement. Read-only queries do not acquire native source connections.

The [DuckDB batching plan](../../docs/duckdb-batching-plan.md) records where
the current implementation still executes SQL per source or row, and how to
replace those paths while preserving these transaction boundaries.

## Growth points

Add `claude_code`, `opencode`, `pi` and `tau` persistence modules when their native
table implementations exist. Keep typed native fields/content references and
JSON extras with that owner. Shared history concepts belong to history/types.
The content archive is implemented for live Codex/Claude capture. The proposed
general graph, resolved prefix bindings and mutable SQLite capture remain fixture-led
follow-up work. Existing native metadata tables live in harness storage.

Keep FTS/vector SQL with history until another ownership/dependency boundary is
useful. Reassess this layout periodically. The application owns capture
and replay sequencing. History supplies retained records and atomically publishes
prepared inputs after checking their expected checkpoint.

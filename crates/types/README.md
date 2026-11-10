# llm-history-types

Shared history values used by ingestion, persistence and application callers.

- `profile`: `Harness` and `Profile` identity/configuration values.
- `fact`: the existing decoded `Fact` contract.
- `usage`: usage accounting values.
- `canonical`: shared session and native-record metadata values.
- `provider`: timestamp, context and token metadata values.
- `query`: selections, filters, messages, search modes and search requests.
- `archive`: retained-source filters/results and archive/restore receipts.
- `ingestion`: import, catalog and replay receipts.
- `storage`: resource options and the lock-contention error value.
- `jobs`: indexing progress receipts.
- `catalog`: captured project/assignment metadata.
- `native::codex`: selected captured thread metadata shared by source acquisition
  and native persistence.

This is the **2026-11-07 extraction snapshot**. Existing fields and Serde attributes
are preserved, including historical metadata names. The model currently serves
Codex/Claude ingestion; future harness semantics and richer content/graph values
need deliberate assessment against fixtures.

Native decoding and parser checkpoints belong to `llm-history-ingest`. Capture,
search/output policy and timestamp interpretation currently remain in `core`.
Persistence owns physical rows and transactions. This crate has no workspace
dependencies and performs no acquisition, normalization or storage operations.

Import shared values directly from their defining modules in `llm_history_types`.
Architecture checks reject foreign public re-exports across the workspace,
wildcard public exports and I/O/backend dependencies here. Physical row identifiers
stay private to storage; the public retained-source response excludes its internal
generation key. Resource defaults are identical in production and tests; tests
configure smaller batches explicitly when required.

# Repository instructions

- Source history and Python reference are read-only. Use synthetic fixtures in Git.
- Rust toolchain: beta-2026-10-18. Resolve dependencies behind the three-day Cargo age gate; commit Cargo.lock and mise.lock. Review dependency and native artifact changes before execution.
- Check formatting and run tests covering the changed behavior. Use workspace Clippy/tests for shared-interface or storage changes. Reuse completed checks while the relevant code is unchanged. Build release binaries when delivering one or measuring release performance.
- Use mise tasks for standard builds/checks (`build`, `release`, `test`, `lint`, `check`), and `mise exec -- cargo ...` for focused commands. Cargo owns incremental compilation; keep compiler flags in the Cargo configuration.
- Keep database code out of parsers, CLI code out of libraries, and process/HTTP details behind model interfaces.
- Keep store identity in every native-ID lookup, relationship, filter, and context query.
- No automatic downloads during ordinary queries. Check artifact SHA-256 once, when downloading; `models verify` re-checks on request. Runtime paths only check that files exist.
- Hash once: blake3 identifies each raw payload at capture; xxh64 is for change detection (head/boundary windows) and `raw verify`. Unchanged size+mtime skips a source without reading it. Do not add re-hashing to update, search, open, import or reprocess paths.
- Keep XXH64 fingerprints as unsigned 64-bit integers: Rust `u64`, DuckDB `UBIGINT`, and exact integer tokens in JSON. Use integer bindings, storage and comparisons. Convert to text only when an external format explicitly requires it.
- Embedding vectors are native DuckDB `FLOAT[]` lists, scored in SQL (`list_cosine_similarity`) in one query. Never store vectors as text or score them row by row in Rust. Embedding requests are batched, and token counts use the pinned local tokenizer when it is cached.
- Databases upgrade themselves on open, in three ways (`crates/store/history/src/schema/mod.rs`). A change to a table that holds captured or assigned data, such as the raw archive, short IDs or embeddings, adds one step to `MIGRATIONS` and raises `SCHEMA_VERSION`; never edit a merged step. Views and macros hold no data: edit their SQL files and every build replaces them with its own. Parsed facts are rebuilt from the archive by raising `PARSER_VERSION`. Raise `SCHEMA_VERSION` without a table change only when an older build would misread the database. Keep one-off conversions of databases older than 0.1.0, including Python archives, in ignored local DuckDB SQL.
- No provenance records, attestations, or tests of the toolchain itself unless asked.
- Report the commands and results actually observed. Keep private test data outside Git.
- Commit messages and pull request descriptions carry no tool attribution: no `Co-Authored-By` line for an AI assistant and no "generated with" line.
- `docs/architecture.md` is a dated descriptive assessment, with candidate evolution separated from the implemented layout. Reassess boundaries as features grow. Selected executable rules live in `tools/architecture/policy.toml`; `mise run architecture` checks them and is included in `check`/`test`. Update relevant rules and rationale when deliberately changing ownership; retire resolved legacy exceptions.
- Native durable-format references live in `docs/harness-history/`. Consult the matching harness notes for parser/acquisition changes; distinguish saved records from runtime/API projections. A new upstream review should record its date, exact checkout and public source permalinks, preserving explicit uncertainties and synthetic examples.

## DuckDB batch performance

- Pass bounded sets of work to DuckDB. Bulk paths must not execute a SQL lookup or mutation for each record, session, or source, even inside a shared transaction. Stage rows with an Appender or typed parameters, apply changes in sets, and skip unchanged writes.
- Filter membership with semi joins and absence with anti joins, or equivalent `IN (SELECT …)` and `NOT EXISTS` predicates. Keep profile and source keys in the join, preserve null semantics, and push selection and limits into SQL before hydrating results.
- Measure expensive SQL executions and rows revisited as the input grows. A Rust loop that decodes an ordered transcript or feeds a bounded Appender is expected; repeated SQL execution and rescanning old history are performance defects. See the [DuckDB batching plan](docs/duckdb-batching-plan.md).

## Refactoring and API ownership

- Prefer one canonical definition and public import path for every symbol. Update all internal consumers when relocating code and remove the old interface.
- Internal Rust API changes are acceptable in this unreleased application. Preserve the explicitly documented CLI and captured-data behavior contracts.
- Do not introduce compatibility shims, forwarding wrappers or aliases to retain obsolete internal interfaces. Tests must use the intended current operations; update or remove tests of obsolete interfaces.
- Use `pub use` only for deliberate same-crate public facades over private modules. Import foreign symbols from their owning crate directly; avoid wildcard public exports.
- A refactor includes obsolete code, imports, tests and documentation cleanup. Optimize for the simplest final codebase, rather than minimizing the diff.

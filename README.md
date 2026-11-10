# llm-history

Captures, archives and searches local Codex, Claude Code, and Cursor history. This Rust implementation of `llm-history` keeps its own database, separate from the Python implementation (reference `a7aa830`). JSONL transcripts are archived byte for byte, so history survives after the agents delete their files.

## Development

```sh
mise run build        # development executable
mise run test         # architecture and workspace tests
mise run check        # architecture, formatting, Clippy and tests
mise run format       # format Rust, maintained docs and TOML
mise run hooks:install # install the Git pre-push formatting check
mise run architecture # focused pytest checks for architectural boundaries
mise run release      # optimized executable (use for timing)
```

Install the pinned development tools with `mise install --locked`. The pre-push
hook runs formatting checks; `mise run check` also runs Clippy, architecture
checks and the workspace tests. `mise run hooks:check` runs the hook locally.
The docs formatter covers maintained Markdown and selected config files.
Native fixtures, generated catalogs and vendor sources stay outside its scope.

The toolchain is pinned in `rust-toolchain.toml`. Cargo enforces a three-day publish age for dependencies; `mise run lock` refreshes `Cargo.lock` and checks it against OSV. Use the release executable to measure end-to-end performance.

On macOS and Linux, `mise run release` produces `target/release/llm-history`.
Run that file directly or install it on your PATH. If you prefer a shorter
interactive command, add `alias llmh=llm-history` to your shell configuration.

On native x64 Windows, install Rust with `rustup`, then run `mise install --locked`,
`mise run setup:windows`, and `mise run release`. Setup prompts for the
Microsoft license and installs the kit selected by `portablemsvc.lock` in
PortableMSVC's per-user location. Builds, Clippy and tests activate that kit
for the command only. The optimized executable is `target\release\llm-history.exe`.
Use the native Windows build of mise for this path. On macOS, use the Apple
Command Line Tools for ordinary builds.

To update the Windows kit pin, first install and review the replacement kit
with PortableMSVC. Pass its generated lockfile to
`mise exec -- python scripts/export_portablemsvc_lock.py PATH_TO_INSTALLED_LOCK`.
This keeps the Microsoft package pins and strips local paths before writing
the repository lockfile. Review the diff and verify a Windows build.

For a one-off build, set `CARGO_TARGET_DIR` to a temporary directory and copy
the executable out of its `release/` directory before removing it. For regular
development, `mise exec -- cargo clean --profile dev` reclaims debug and test
build output while keeping the release build. Inference runtime and model
downloads are separate, optional steps.

The storage design calls for bounded sets of work in DuckDB so SQL executions
scale with batches. See the [DuckDB batching plan](docs/duckdb-batching-plan.md)
for the current findings, fixes and verification criteria.

Local inference (optional) uses pinned llama.cpp binaries and models from `artifacts.json`:

```sh
mise run setup-native
llm-history models fetch --download-model
```

Models live in the shared Hugging Face cache; the llama.cpp runtime lives in `~/.cache/llm-history-rs` (`--cache`, `LLM_HISTORY_RS_CACHE`). `scripts/evaluate_models.py` runs the retrieval evaluation over `fixtures/retrieval.json`.

## Use

Library callers use the `llm-history-app` crate's `History` handle. The handle takes
an explicit database path, holds no connection between operations, and exposes
profile management, import/replay, typed queries, search, raw export and maintenance.
The CLI lives in `crates/cli`; its database operations use this application interface.
See [the application contract](docs/application-api.md) and
[public synthetic acceptance fixtures](fixtures/contracts/scenario.json).
The [architecture assessment](docs/architecture.md) records the current shape,
candidate evolution, executable guardrails and when to reassess the boundaries.
The [harness history reference](docs/harness-history/README.md) records native
durable formats, lifecycle behavior and pinned upstream sources for Codex,
Claude Code, OpenCode, Pi and Hugging Face Tau.
Native envelope readers now live in [the ingestion adapter crate](crates/ingest/README.md),
`llm-history-ingest`, with a module per harness and internal shared JSON/presence
primitives. Codex/Claude decoding and checkpoints now live there and serve live
import and archive replay. Shared fact, identity and metadata values live in
[`llm-history-types`](crates/types/README.md); callers import those types directly.
Application requests/results live in explicit `llm_history_app::api` modules.
Cross-crate forwarding exports and wildcard public exports are rejected by the
architecture checks. Archive-file operations have one API in
`llm_history_store::archives`; database workflows go through `History`.
The [storage crate family](crates/store/README.md) separates engine/native-build
ownership, shared history persistence and harness-specific table persistence.

Run the application/CLI contract tests with `mise run test`. For a temporary
before/after CLI comparison, preserve an earlier executable and run:

```sh
LLM_HISTORY_BASELINE=/path/to/preserved/llm-history mise exec -- \
  cargo test -p llm-history-cli --test application_contract --locked -- --nocapture
```

Both sides run on isolated synthetic stores; the comparison covers structured and
piped output, errors, profile scope, append/replay, usage, search and raw export.

```sh
llm-history init                      # default Codex and Claude profiles
llm-history profile add work --harness codex --root ~/.codex-work
llm-history update
llm-history search 'repair stale history' --profile work
llm-history search --mode exact 'git push --force'   # literal string, newest first
llm-history show MESSAGE_ID --before 2 --after 2
llm-history similar MESSAGE_ID --limit 10
llm-history similar MESSAGE_ID --role user --other-sessions --per-session 1
llm-history sessions
llm-history usage --by-model
llm-history sql 'select * from history_help'
```

The database defaults to `~/.local/share/llm-history-rs/history.duckdb` (`--db`, `LLM_HISTORY_RS_DB`). `--json` gives machine-readable output. Search is lexical by default; `--mode semantic|hybrid` and `--rerank` use the local or `--endpoint` models. `--mode exact` needs no model. When limits omit matches, JSON includes the total `matched` count; the result array gives the count shown. `--kind interactive|subagent|automation|guardian|unknown` limits `search` and `sessions` to one session kind, including hidden subagent and guardian sessions; guardian search is lexical only, and each hit carries the review's `verdict`. Search checks source metadata and refreshes when it detects a change. Unchanged sources stay on the read path. Use `--no-refresh --no-background` to only read the archive.

`search`, `show`, `similar`, and `sessions` use short integer IDs scoped to the
database. Use the `id` from a search hit with `show` or `similar`. Their JSON
keeps message text and scope; select `message_no`, `session_no`, and native
metadata explicitly through SQL when tracing source records.

`similar` compares stored document embeddings and searches the message's
profile by default. Pass `--profile` to select other profiles explicitly.
For recurring complaints, filter `--role user`, exclude the anchor's thread
with `--other-sessions`, and open candidates with `show` before counting incidents.
It uses the 768-dimensional Gemma 2 index by default (`--dimensions 256` for
the separate 256-dimensional index). The command doesn't refresh sources or
run inference; an unindexed message needs `semantic backfill` first.

`sql --limit` caps displayed rows. Put a `LIMIT` in the SQL to constrain the selected rows. DuckDB execution is interrupted after 15 seconds by default; set `sql --timeout-seconds` for a different deadline. The deadline also applies with `--full`.

Profiles are independent stores. Codex profiles read `sessions/` and `archived_sessions/` (including `.jsonl.zst`); Claude profiles read `projects/`. Native IDs are scoped to their profile.

To opt in to Cursor, register its data root (normally `~/.cursor`):

```sh
llm-history profile add cursor --harness cursor --root ~/.cursor
llm-history update --source cursor
llm-history search --source cursor 'a previous decision'
```

Cursor profiles read local agent JSONL transcripts and CLI chat databases
under `chats/`. When both describe the same session, the CLI database supplies
the selected conversation and tool results. A failed snapshot leaves the
previous archive intact. Cursor's desktop `state.vscdb` and cloud history are
outside this importer; editor chats with local JSONL are supported.

SQL output bounds headers and text cells with `--max-cell-bytes`, and the complete JSON result with `--max-output-bytes`. Shortened headers use the same names in `columns` and row keys, with distinct suffixes when prefixes collide. JSON reports `truncated`, `truncation_reasons` and `truncated_headers`; `sql --full` returns complete headers and values.

SQL connections include `llm_history_blake3(BLOB)` for a 32-byte digest and
`llm_history_xxh64(BLOB)` for a seed-zero `UBIGINT` fingerprint. XXH64
stays unsigned through storage and SQL; JSON emits an exact integer token.
Clients without native 64-bit JSON integers should parse it losslessly. Use
`encode(text)` to hash UTF-8 text and `lower(hex(digest))` to display BLAKE3.

## Storage

- Native observations and captures retain source positions. Native documents retain the original JSON layout, with selected text and tool strings shared through BLAKE3 content objects. Replay and raw export reconstruct the exact captured bytes. Cursor SQLite exports contain retained rows as JSONL, rather than a copy of the SQLite database. Sources that disappear are flagged `missing` and stay searchable.
- Each parsed fact is one row of `events`. Python-compatible `canonical_*`, `provider_event_details` and `history_*` surfaces are views. Turn contexts, token counts, epochs, relations, conversation groups and projects are their own tables.
- `update` skips files whose size and mtime are unchanged and captures changed files together in batched transactions. `reprocess` rebuilds the projection from the archive with stable IDs. New databases use schema 1. Before release, convert earlier development databases offline or use a new `--db`.
- `db compact` rewrites the file to reclaim space. `raw verify` checks the archive on request; nothing re-verifies it during normal work.
- Short message and session numbers are assigned once, separately from archive keys. A restored snapshot carries the numbers recorded in that snapshot. Earlier schema 1 archives need an offline backfill on a copy before use with this version.

Parity with Python is tracked in [docs/parity.md](docs/parity.md).

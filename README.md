# llm-history

`llm-history` archives local Codex, Claude Code and Cursor history and searches it. It archives JSONL transcripts byte for byte, so the history survives after the agents delete their files. This Rust implementation keeps its own database, separate from the Python implementation's. Parity is measured against Python commit `a7aa830`.

## Use

Build the executable first with `mise run release`, as described under [Development](#development).

### From the command line

```sh
llm-history init                      # default Codex and Claude profiles
llm-history profile add work --harness codex --root ~/.codex-work
llm-history update
llm-history search 'stale repair' --profile work      # your messages with every word, newest first
llm-history search --mode exact 'git push --force'   # the whole query as one literal string
llm-history search --in tools 'git push --force'     # commands the agents ran
llm-history search --mode lexical --role all 'repair stale history'   # ranked, answers included
llm-history show MESSAGE_ID MESSAGE_ID                # each with two messages either side
llm-history show --session SESSION_ID                # your turns in order, then the last answer
llm-history similar MESSAGE_ID --limit 10
llm-history similar MESSAGE_ID --role user --other-sessions --per-session 1
llm-history sessions --under ~/projects/demo
llm-history usage --by-model
llm-history sql 'select * from history_help'
```

The database defaults to `~/.local/share/llm-history-rs/history.duckdb`. Change it with `--db` or `LLM_HISTORY_RS_DB`. `--json` prints machine-readable output. `--help` lists a command's common options with examples first, and `--help-all` lists every option.

[`skills/history-recall`](skills/history-recall/SKILL.md) is a skill that teaches a coding agent these commands. Copy the folder into `~/.codex/skills` or `~/.claude/skills`, and replace it when you upgrade the executable.

#### Search

By default, `search` covers the messages you typed and uses `--mode words`. Every word of the query must appear in the message as a case-insensitive string, in any order, and hits are newest first. Each word also matches inside longer words, so `orchestrat` finds `orchestrate` and `orchestration`. Search returns up to 8 hits and 8,000 characters of message text. Raise those limits with `--limit` and `--max-output-chars`.

To find a command an agent ran, add `--in tools`. It searches the arguments of tool calls, such as shell commands, with `--mode words` or `--mode exact`. Each hit names the tool and gives the ID of the message before the call, which `show` opens.

Other modes and roles:

- `--mode exact` matches the whole query as one literal string.
- `--mode lexical` ranks messages that contain any of the words.
- `--mode semantic` and `--mode hybrid` use the local embedding model, or the server at `--endpoint`.
- `--rerank` reorders hits with the local reranking model, or the server at `--rerank-endpoint`. It needs `--mode lexical`, `semantic` or `hybrid`.
- `--role assistant` searches final answers, and `--role all` searches both roles. With `--role all`, a ranked mode shows the answer to a matching question.

When a limit cuts off `words` or `exact` matches, JSON includes the total as `matched`. The length of the result array is the number shown. A search with no hits prints one line with the number of messages it covered and the time of the newest archived message. JSON carries the same two values as `searched`.

`--since` and `--until` take an ISO date or timestamp. `--after` and `--before` mean the same, as in `git log`.

`--kind interactive|subagent|automation|guardian|unknown` limits `search` and `sessions` to one kind of session. Naming `subagent` or `guardian` includes those sessions even where they are otherwise hidden. Each guardian hit carries the review's `verdict`. Guardian reviews have no embeddings, so `--mode semantic` rejects them.

Search checks the metadata of the source files first and imports changes when it finds any. When nothing changed, it only reads the database. Pass `--no-refresh --no-background` to search the archive as it is.

#### Message and session IDs

`search`, `show`, `similar` and `sessions` use short integer IDs that are local to the database. Pass the `id` from a search hit to `show` or `similar`. A search hit also prints its `session`, which `show --session` takes. The SQL views use the same IDs: `message_id` and `session_id` in every `history_*` view are these short numbers. The long archive keys are `event_id` and `session_key`. JSON from `search` and `similar` gives each hit's ID, session, profile, project, role, timestamp and text. To trace a message to its source records, select `event_id`, `session_key` and the native metadata columns through SQL.

#### Show

`show` takes up to 10 message IDs and prints each with two messages before and after it. Change that with `-B`, `-A` or `-C`, as in `grep`. All the IDs share one budget of 12,000 characters, set by `--max-output-chars`. A short message leaves its share to the long ones, and each requested message gets up to two thirds of its block. Messages whose context overlaps print once, in one block. `--full` removes the budget. With one ID, JSON is a single context object. With several, the objects are in a `contexts` array.

`show --session SESSION_ID` reads one session. It prints the user turns in conversation order, numbered, and then the session's last final answer. Three limits apply:

- A turn over 1,000 characters keeps its first 700 and last 300, with the number of characters cut between them. The last answer has the same rule at 2,000 characters.
- The turns share `--max-output-chars`, 12,000 by default. A session over that budget keeps its first three turns and as many of its latest turns as fit. One line states which turns it skipped and their IDs.
- There is no `--full` for a session. Raise `--max-output-chars`, pass `--from ID` to start at a skipped turn, or open a single turn with `show ID --full`.

`--recent N` prints the opening turn and the latest N turns whatever the session's size.

`sessions` lists recent sessions and takes the same `--project`, `--cwd` and `--under` filters as `search`.

#### Similar messages

`similar` compares stored document embeddings. It searches the message's own profile unless you pass `--profile`. It uses the 768-dimensional Gemma 2 index by default, and `--dimensions 256` selects the separate 256-dimensional index. The command doesn't refresh sources or run inference, so a message without an embedding needs `semantic backfill` first.

To find a complaint that recurs, pass `--role user` and `--other-sessions`, which excludes the session the message came from. Open each candidate with `show` before you count it as a separate incident.

#### SQL

`sql --limit` caps the rows displayed. Put a `LIMIT` in the SQL to cap the rows selected. `sql` interrupts DuckDB after 15 seconds by default, including with `--full`. Set `--timeout-seconds` for a different deadline.

`--max-cell-bytes` bounds each header and text cell at 2,000 bytes by default. `--max-output-bytes` bounds the complete JSON result at 16,000 bytes by default. A shortened header has the same name in `columns` and in the row keys, and shortened headers that would collide get distinct suffixes. JSON reports `truncated`, `truncation_reasons` and `truncated_headers`. `sql --full` returns complete headers and values.

SQL connections include `llm_history_blake3(BLOB)`, which returns a 32-byte digest, and `llm_history_xxh64(BLOB)`, which returns a seed-zero `UBIGINT` fingerprint. XXH64 values stay unsigned through storage and SQL, and JSON emits them as exact integer tokens. A client without native 64-bit JSON integers has to parse them losslessly. Use `encode(text)` to hash UTF-8 text and `lower(hex(digest))` to display a BLAKE3 digest.

#### Profiles

Each profile is an independent store, and native IDs are scoped to their profile. Codex profiles read `sessions/` and `archived_sessions/`, including `.jsonl.zst` files. Claude profiles read `projects/`.

Cursor is opt-in. Register its data root, normally `~/.cursor`:

```sh
llm-history profile add cursor --harness cursor --root ~/.cursor
llm-history update --source cursor
llm-history search --source cursor 'a previous decision'
```

Cursor profiles read local agent JSONL transcripts and the CLI chat databases under `chats/`. When both describe the same session, the CLI database supplies the selected conversation and the tool results. A failed snapshot leaves the previous archive intact. The importer reads editor chats that have local JSONL. It does not read Cursor's desktop `state.vscdb` or cloud history.

### As a library

Library callers use the `History` handle from the `llm-history-app` crate. The handle takes an explicit database path and holds no connection between operations. It covers profile management, import and replay, typed queries, search, raw export and maintenance. The CLI in `crates/cli` does its database work through the same handle.

- [The application contract](docs/application-api.md) describes the handle. [`fixtures/contracts/scenario.json`](fixtures/contracts/scenario.json) is the public synthetic acceptance scenario.
- [The architecture assessment](docs/architecture.md) records the current crate layout, changes under consideration, the rules the architecture checks enforce and when to reassess the boundaries.
- [The harness history reference](docs/harness-history/README.md) records the formats each tool saves to disk, their lifecycle behavior and pinned upstream sources for Codex, Claude Code, OpenCode, Pi and Hugging Face Tau.
- [`llm-history-ingest`](crates/ingest/README.md) reads each harness's native records, with one module per harness. Its shared JSON and presence helpers are internal to the crate. It decodes Codex and Claude records and their checkpoints for both live import and archive replay.
- [`llm-history-types`](crates/types/README.md) holds the shared fact, identity and metadata types. Callers import them from that crate directly.
- `llm_history_app::api` holds the application request and result types.
- `llm_history_store::archives` is the one API for archive files. Database workflows go through `History`.
- [The storage crates](crates/store/README.md) are an engine crate that owns DuckDB and its native build, a history crate for the shared tables, and a harnesses crate for harness-specific tables.

The architecture checks reject cross-crate forwarding exports and wildcard public exports.

`mise run test` runs the application and CLI contract tests. To compare the CLI before and after a change, keep a copy of the earlier executable and run:

```sh
LLM_HISTORY_BASELINE=/path/to/preserved/llm-history mise exec -- \
  cargo test -p llm-history-cli --test application_contract --locked -- --nocapture
```

Both executables run on isolated synthetic stores. The comparison covers structured and piped output, errors, profile scope, append and replay, usage, search and raw export.

## Development

A development build uses your real database unless you pass `--db` or set `LLM_HISTORY_RS_DB`, and it upgrades that database the first time it opens it. An older build then refuses the database. Before you try a development build, save a copy with `llm-history archive create`, or point the build at a scratch database.

```sh
mise run build        # development executable
mise run test         # architecture and workspace tests
mise run check        # architecture, formatting, Clippy and tests
mise run format       # format Rust, maintained docs and TOML
mise run hooks:install # install the Git pre-push formatting check
mise run architecture # focused pytest checks for architectural boundaries
mise run release      # optimized executable (use for timing)
```

Install the pinned development tools with `mise install --locked`. The pre-push hook checks formatting only. `mise run check` also runs Clippy, the architecture checks and the workspace tests. `mise run hooks:check` runs the hook without pushing. The docs formatter covers the maintained Markdown and a few config files. It skips native fixtures, generated catalogs and vendored sources.

`rust-toolchain.toml` pins the toolchain. Cargo only resolves dependency versions published at least three days ago. `mise run lock` refreshes `Cargo.lock` and checks it against OSV. Measure end-to-end performance with the release executable.

On macOS and Linux, `mise run release` produces `target/release/llm-history`. Run that file directly or install it on your PATH. For a shorter interactive command, add `alias llmh=llm-history` to your shell configuration. On macOS, the Apple Command Line Tools are enough for ordinary builds.

On native x64 Windows, use the native Windows build of mise. Install Rust with `rustup`, then run `mise install --locked`, `mise run setup:windows` and `mise run release`. Setup prompts for the Microsoft license and installs the kit selected by `portablemsvc.lock` in PortableMSVC's per-user location. Builds, Clippy and tests activate that kit for the duration of the command. The optimized executable is `target\release\llm-history.exe`.

To update the Windows kit pin, first install and review the replacement kit with PortableMSVC. Pass its generated lockfile to `mise exec -- python scripts/export_portablemsvc_lock.py PATH_TO_INSTALLED_LOCK`. The script keeps the Microsoft package pins and strips local paths before it writes the repository lockfile. Review the diff and verify a Windows build.

For a one-off build, set `CARGO_TARGET_DIR` to a temporary directory and copy the executable out of its `release/` directory before you remove it. For regular development, `mise exec -- cargo clean --profile dev` deletes debug and test build output and keeps the release build. The inference runtime and the models are separate, optional downloads.

Storage code passes work to DuckDB in bounded batches, so the number of SQL statements grows with the number of batches and not with the number of records. The [DuckDB batching plan](docs/duckdb-batching-plan.md) lists the current findings, fixes and verification criteria.

Local inference is optional. It uses the pinned llama.cpp binaries and models listed in `artifacts.json`:

```sh
mise run setup-native
llm-history models fetch --download-model
```

Models go in the shared Hugging Face cache. The llama.cpp runtime goes in `~/.cache/llm-history-rs`, which you can change with `--cache` or `LLM_HISTORY_RS_CACHE`. `scripts/evaluate_models.py` runs the retrieval evaluation over `fixtures/retrieval.json`.

## Storage

- Native observations and captures keep the position of every record in its source. Native documents keep the original JSON layout. The archive stores selected text and tool strings once, as content objects keyed by BLAKE3 hash. Replay and raw export reconstruct the exact captured bytes. A Cursor SQLite export contains the retained rows as JSONL, not a copy of the SQLite database. When a source disappears, the archive flags it `missing` and keeps it searchable.
- Each parsed fact is one row of `events`. The Python-compatible `canonical_*`, `provider_event_details` and `history_*` names are views. Turn contexts, token counts, epochs, relations, conversation groups and projects have their own tables.
- `update` skips files whose size and mtime are unchanged and captures the changed files together in batched transactions. `reprocess` rebuilds the parsed facts from the archive and keeps IDs stable.
- Two versions describe a database, and both are constants in this repository. `SCHEMA_VERSION` in `crates/store/history/src/lib.rs` is the table layout, which a database records once. `PARSER_VERSION` in `crates/ingest/src/lib.rs` is the decoder revision, which a database records for every source. `status` prints them as `schema_version` and `parser_version`.
- A new build upgrades a database the first time it opens it, and no command is needed. It applies any table-layout steps the database is missing, in order, and replaces the views and macros with its own. Schema 2 changed no table. It marks the point where `message_id` and `session_id` in the views became the short numbers, so a 0.1.0 executable refuses a schema 2 database where it would otherwise misread those columns. A build also refuses a database with a newer schema than its own.
- A new decoder revision needs no manual step either. The next `update` or refreshing `search` rebuilds the parsed facts of every source from the archive, and `status` counts the sources still waiting as `pending_parser_upgrades`. Message IDs, message text and embeddings stay as they were. On a 3.9 GiB archive of 786,000 records, the rebuild from revision 6 to 7 took 41 seconds. The command that starts a rebuild prints one line on stderr first.
- `db compact` rewrites the file to reclaim space. `raw verify` checks the archive when you run it. Nothing re-verifies the archive during normal work.
- The store assigns each short message and session number once, separately from archive keys. A restored snapshot carries the numbers recorded in that snapshot. A development database from before 0.1.0 that lacks these numbers needs an offline backfill on a copy first.

[docs/parity.md](docs/parity.md) tracks parity with Python.

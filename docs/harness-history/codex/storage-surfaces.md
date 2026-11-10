# Codex storage surfaces and SQLite schemas

Reviewed **2026-11-07**, `openai/codex` `main` at
**`2bc8e7f5459b084afa482c7aac3f56fef36286b3`**. This section inventories local
durable conversation inputs, metadata and adjacent stores. It distinguishes
source transcripts from query projections and product state. It uses public
source and fresh in-memory migration databases, with no personal history access.

[storage-schema.json](storage-schema.json) contains the resulting schema for all
six public SQLite migration families: **27 final application tables**, including
columns, types, defaults, primary-key positions, foreign keys, indexes, triggers
and complete final table DDL. It links every applied SQL migration. The seven
runtime database filenames include two memory versions using the same migrations.
SQLx's own migration bookkeeping and runtime-created data are outside that
fresh-DDL inventory. This describes a newly migrated database at the pin;
older/mixed-version files can have other migration states.
[Migrators][migrators], [runtime file definitions][sqlite].

## Shared thread types and store boundary

At this source revision, `ThreadStore` is the shared persistence/query interface.
Its operations cover thread creation/resume, metadata changes, listing/history,
fork preparation, archive/unarchive/revert/delete, projects, sections and
attachments. `StoredThread` is a typed result carrying thread/fork/spawn identity,
location and metadata; full history is optional and requested separately.
[Store trait][thread-store], [stored result][stored-thread].

`state::ThreadMetadata` models persisted SQLite discovery metadata, including the
selected rollout path, history mode, names, model/settings, archive state and
project/section associations. Rollout `SessionMeta` supplies native family/thread
identity, logical fork/execution parent and physical prefix boundaries. These types
and surfaces complement each other. A global catalog can determine which of several
files is selected, while the file header retains relationship details the catalog
does not reproduce. [Metadata model][metadata], [native session metadata][session-meta].

The configured builder creates local or in-memory stores. Local metadata paths and
rollouts must be interpreted with the local resolver/lifecycle rules. The public
source does not establish closed-host backing-store internals.
[Configured builder][store-builder], [backend review](readers-and-lifecycle.md).

## Files and discovery

| Surface | Saved material | Writer / reader |
| --- | --- | --- |
| `sessions/YYYY/MM/DD/rollout-*.jsonl` | Active physical rollout; LF-delimited timestamped native records | Recorder and thread-store creation; discovery/listing and rollout readers |
| `archived_sessions/rollout-*.jsonl` | Archived physical rollout | Archive/unarchive lifecycle and include-archived readers |
| `.jsonl.zst` representation | Compressed bytes of the same decoded rollout stream | Compression/materialization and plain/compressed rollout reader |
| `session_index.jsonl` | Append-only `{id,thread_name,updated_at}` names; later entries win | Session-index writer, reverse lookup readers and rewrite-on-removal |
| `history.jsonl` | Prompt recall `{session_id,ts,text}`; integer Unix seconds | Message-history append and bounded lookup/batch readers |
| `state_5.sqlite` | Current rollout selection, discovery metadata, projects, spawn graph and product state | State-runtime methods and thread-store metadata sync |
| `thread_history_1.sqlite` | Paginated projected turns/items/realtime presentation and replay position | Thread-history projector and paginated readers |

Creation paths are based on local creation date/time with UTC fallback. Legacy
flat filenames remain discoverable. A replacement filename can contain a separate
physical rollout ID after the logical thread ID. Header metadata, filename identity
and current SQLite path selection therefore all matter. Plain/compressed siblings
are representations of one source rather than two conversations.
[Path writer][recorder], [filename parser][filenames], [discovery][discovery],
[compression][compression], [current resolver][resolver].

`session_index.jsonl` uses RFC3339 update timestamps. Name updates are appended;
removal serializes a rewrite through a separate index lock. Malformed lines are
retained or skipped according to the operation. An explicit user-selected name
is independent of a rollout's first-user-message title. [Session names][names].

Prompt recall has its own persistence setting and optional size budget. Appending
can be disabled; over-budget trimming targets a soft cap. It represents submitted
prompt recall rather than the complete response/tool conversation. Its `session_id`
is written from the caller's supplied conversation ID; interpret it in that API's
scope rather than assuming every field with that spelling is the rollout's agent
family identity. [Prompt-history writer and entry][prompt].

The header-reference index is rebuilt in memory from rollout paths/metadata for
dependency-sensitive operations. It is not an additional persistent graph file.
[Reference index][references].

## Locations, journals and versions

Configuration resolves `sqlite_home` from explicit configuration, then
`CODEX_SQLITE_HOME`, then the Codex home, subject to managed requirements. The
environment path resolves relative to the configured working directory. A configured
log directory can select a separate logs database home; otherwise the logs database
uses the SQLite home. Never assume the SQLite files sit beside the rollout files.
[Configuration][config], [SQLite configuration][sqlite].

Runtime files and migration families are:

| File | Migration family | Role |
| --- | --- | --- |
| `state_5.sqlite` | `state/migrations` | Current metadata and product-owned relationships/state |
| `thread_history_1.sqlite` | `state/thread_history_migrations` | Projected conversation paging |
| `logs_2.sqlite` | `state/logs_migrations` | Bounded diagnostic feedback logs |
| `goals_1.sqlite` | `state/goals_migrations` | Goal accounting and continuation deferrals |
| `memories_1.sqlite`, `memories_v2_1.sqlite` | `state/memory_migrations` | Memory extraction/consolidation versions |
| `queue_1.sqlite` | `state/queue_migrations` | Pending durable user submissions |

Writable connection policy chooses WAL away from NFS and DELETE for new NFS
databases, preserving existing settings unless enforcement requires migration.
Committed WAL contents can matter during read-only acquisition. Migrations and
version checks execute transactionally with busy retries. The runtime migrator
accepts unknown applied newer migration versions while still checking known
checksums. State recovery can back up/rebuild selected databases; paginated history
uses an unavailable-on-failure policy. Recovery backups are not additional current
transcripts. [Connection/recovery policies][sqlite], [version compatibility][migrators].

## Primary state: 15 tables

The complete 43-column `threads` declaration is in the JSON inventory. Grouped by
meaning, it contains:

- Identity/location: `id`, `rollout_path`, `history_mode`, `attached_thread_parent_id`.
- Time: `created_at`, `updated_at`, `archived_at`, the `_ms` creation/update mirrors,
  and `recency_at`/`recency_at_ms`. Archive time is seconds; millisecond sort columns
  preserve additional ordering and are not interchangeable with creation time.
- Creator/source: `source`, `thread_source`, `originator`, `creator_user_id`,
  `creator_account_id`, `agent_nickname`, `agent_role`, `agent_path`, `cli_version`.
- Current interpretation/settings: `model_provider`, `model`, `reasoning_effort`,
  `cwd`, `sandbox_policy`, `approval_mode`, `tokens_used`, `memory_mode`.
- Discovery/presentation: `title`, `name`, `preview`, `first_user_message`,
  `has_user_event`, `archived`, `is_pinned`, section/project selection and ordering,
  `daybreak_enabled`, and Git SHA/branch/origin.

The SQLite row parser reads millisecond timestamps, converts ID/time values, parses
history mode, and joins section names/appearance. Invalid section joins are errors;
an unrecognized reasoning-effort value becomes absent. Empty preview/first-message
strings normalize to absent values. `source` and `approval_mode` store plain strings
when serde emits a string, and serialized JSON text for structured enum values.
`sandbox_policy` receives the serialized permission profile in the extractor.
[Metadata structure and row decoder][metadata], [metadata extraction][extract].

Metadata extraction ignores embedded ancestor session headers whose logical ID
differs from the selected thread. Several settings/message events update the current
projection, and Guardian prompts are deliberately excluded from first-message
previews. A migration also clears historical Guardian prompt copies from this
projection while retaining canonical rollout JSONL. Metadata rebuildability is
column-specific: user-owned project/section/read state has separate writers.
[Extraction][extract], [Guardian cleanup migration][guardian-cleanup].

| Other state tables | Contents and semantics |
| --- | --- |
| `thread_dynamic_tools` | Ordered declarations: thread, position, name, description, input-schema text, deferred loading and namespace |
| `thread_spawn_edges` | Execution parent, unique child, open/closed lifecycle status; separate from history inheritance |
| `projects`, `project_roots`, `project_idempotency_keys` | Current named project, ordered roots, string-map metadata, ordering/times and create-operation idempotency |
| `thread_sections` | Independent named presentation section and optional appearance JSON |
| `thread_attachments` | Thread-owned attachment identity/type, client-defined JSON metadata, attachment time |
| `thread_read_receipts` | Durable unread boundary/revision and last delivered turn, independent of metadata rebuild |
| `guardian_review_feedback` | Thread-scoped serialized review record BLOB |
| `rollout_migration_state`, `rollout_migration_skipped_rollouts` | Migration progress and source-size/mtime/reason-specific skips |
| `backfill_state` | Singleton metadata backfill status/watermark/time |
| `remote_control_enrollments` | Remote-control enrollment settings |
| `external_agent_config_imports` | Completed external configuration-import results |

Projects are current mutable product state: `threads.project_id` can be reassigned;
project deletion sets it to null, and roots cascade. The tables do not contain
historical membership observations. Project `metadata` is JSON text decoded as a
string-to-string map. [Project model][project-model], [project operations][projects],
[project DDL][project-ddl].

Attachment payloads are JSON values, limited to 64 KiB at this pin; type and identity
key are each limited to 256 bytes and membership to 100 attachments/thread. This
stores attachment metadata, not a universal binary blob store. Fork attachment
membership is copied independently of history cutoff, with fresh attachment row IDs.
Repeated add of an existing type/key preserves its payload/time. [Attachment model][attachments],
[writer/copy behavior][attachment-writer], [limits][limits].

For read state, SQL NULL means read; empty-string `first_unread_turn` means manually
unread from the start. Optimistic revision checks prevent stale acknowledgements.
Last-published turn suppresses redelivery on history replay/revert.
[Read-state operations][read-state], [independent DDL][read-ddl].

## Paginated projection: four tables

| Table | Current fields |
| --- | --- |
| `thread_turns` | `thread_id`, `turn_id`, `rollout_ordinal`, `status`, `error_json`, `started_at`, `completed_at`, `duration_ms`, first/final item IDs, rollout start/end byte positions and end ordinal, `root_turn_id` |
| `thread_items` | Thread/turn/item identity, creation ordinal/time, `item_json`, `item_type`, last-update ordinal, item start/completion milliseconds |
| `thread_history_projection_state` | Thread, next decoded rollout byte offset and ordinal |
| `thread_realtime_items` | Thread/item identity, rollout ordinal, creation milliseconds, item type and JSON |

`item_json` holds serialized app-server `ThreadItemPayload`, including camelCase
variant tags such as `userMessage`. It is a projection of rollout events, distinct
from stored `response_item` JSON. Item identity and creation order can stay stable
while payload and update ordinal change. Lifecycle timestamps retain available
values across updates. The projection checkpoint advances with applied changes;
skipped ordinal ranges keep its byte/ordinal coordinates aligned.
[Projection writer][projection], [payload projector][projector],
[initial projection DDL][projection-ddl], [update ordinal migration][update-ordinal].

An inherited subagent prefix advances projection coordinates without creating child
activity. Fork/revert paging can combine physical ancestor segments. See the
[reader/lifecycle specification](readers-and-lifecycle.md) and
[fork/compaction analysis](forks-and-compaction.md) for selection and replay rules.

## Adjacent durable stores

These are inventoried so they are not accidentally treated as extra transcript
occurrences. Their complete SQL columns/constraints remain in the JSON catalog.

- **Logs:** one final `logs` table, including `feedback_log_body`, timestamp seconds
  and nanoseconds, level/target, source/process/thread metadata and estimated bytes.
  It has pruning budgets and separate recovery. Migration renamed legacy `message`
  into `feedback_log_body`; it is operational diagnostic material.
- **Goals:** `thread_goals` stores objective/status, token budget/usage, time usage
  and timestamps. `thread_goal_continuation_deferrals` records continuation deferral.
- **Memories:** `stage1_outputs`, `jobs`, `consolidation_progress` store derived
  memory/rollout summaries, processing ownership and consolidation readiness. Two
  runtime memory versions share the migration family.
- **Queue:** `queued_items` stores ID, thread, `payload_json`, queue order and times;
  `queued_thread_revisions` and insert/update/delete triggers track changed threads.
  Pending input is a submission rather than evidence of completed conversation.

The queue's payload is externally tagged serde `TurnInput`; the user submission
shape is `{"UserInput":{"content":[...],"client_id":...}}`. Optional
`responsesapi_client_metadata` is injected inside `UserInput` before storage and
removed before typed decoding. The service limits enqueues to user input and
snapshots local attachment inputs. Queue mutations and dispatch can delete/update
rows, so a maximum queued-message ID is not a durable transcript cursor.
[TurnInput][turn-input], [queue codec/service][queue-service],
[queue revision DDL][queue-ddl].

## Source boundaries and preservation

Provider `file_id`, image/audio URLs, opaque encrypted bodies and client-defined
attachment payloads can describe bytes outside these files. The rollout schema
alone cannot establish that the referenced media is locally recoverable. Preserve
native representations and availability evidence as described in
[acquisition/extraction](ingestion.md).

The public source's configured local store and its client-side cloud/protobuf
readers have different responsibilities. Remote pagination cursors are not local
rollout offsets. Closed-client state files such as `.codex-global-state.json` have
no producer schema established by this local source review. They require their own
format evidence; this specification does not silently treat them as canonical.
[Cloud history reader][cloud], [backend boundary](readers-and-lifecycle.md).

Local account-scoped discovery also filters using creator IDs in rollout headers;
older unscoped threads stay shared, and reverted old headers can require consulting
the original rollout. A filtered listing is not proof that omitted files were deleted.
[Account filter][account].

## Rechecking this snapshot

From the llm-history-rs repository, with a Codex checkout at the stated revision:

```sh
uv run --frozen --no-sync --project tools/architecture python \
  tools/architecture/codex_history_docs.py --source <codex-checkout>
```

The checker compares enum inventories with Rust ASTs, replays the public SQL
migrations in fresh in-memory SQLite databases, and validates catalog/example JSON
and source/document references. It reads no live history. It checks documentation
coverage against this source snapshot; it does not execute Codex runtime behavior.
Regenerate the SQL inventory deliberately with `--write-storage-schema` after a
reviewed source-pin change.

[migrators]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/migrations.rs
[sqlite]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/sqlite.rs
[config]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/config/mod.rs#L4399-L4432
[recorder]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/recorder.rs
[filenames]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/rollout_file_name.rs
[discovery]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/list_files.rs
[compression]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/compression.rs
[resolver]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/thread_rollout_resolver.rs
[names]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/session_index.rs
[prompt]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/message-history/src/lib.rs
[references]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/rollout_reference_index.rs
[metadata]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/model/thread_metadata.rs
[extract]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/extract.rs
[guardian-cleanup]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/migrations/0057_cleanup_guardian_thread_metadata.sql
[project-model]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/model/project.rs
[projects]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/runtime/projects.rs
[project-ddl]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/migrations/0049_projects.sql
[attachments]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/model/thread_attachment.rs
[attachment-writer]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/runtime/thread_attachments.rs
[limits]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/lib.rs#L121-L136
[read-state]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/runtime/thread_read_state.rs
[read-ddl]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/migrations/0061_thread_read_state.sql
[projection]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/thread_history.rs
[projector]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/app-server-protocol/src/protocol/thread_history_projection.rs
[projection-ddl]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/thread_history_migrations/0001_thread_history.sql
[update-ordinal]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/thread_history_migrations/0004_thread_items_updated_at_ordinal.sql
[turn-input]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/turn_input.rs#L27-L37
[queue-service]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/ext/queue/src/service.rs#L607-L638
[queue-ddl]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/queue_migrations/0002_queued_thread_revisions.sql
[cloud]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/cloud-client/src/history.rs
[account]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local_history_account.rs
[thread-store]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/store.rs#L129
[stored-thread]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/types.rs#L641-L721
[session-meta]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L3465-L3589
[store-builder]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/thread_store_builder.rs

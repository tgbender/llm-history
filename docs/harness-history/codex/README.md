# Codex durable conversation history

Reviewed **2026-11-07** against `openai/codex`, branch `main`, commit
**`2bc8e7f5459b084afa482c7aac3f56fef36286b3`**. This is a source revision,
not a promise that every installed Codex version has this storage layout.

Scope: durable user/assistant conversation, model-visible reasoning and tools,
context needed to interpret it, and relationships between histories. Streaming
notifications, terminal process logs and execution telemetry are supplementary.
[Wire format](format.md) gives field inventories and authored synthetic examples.
[Acquisition and extraction](ingestion.md) describes capture and CAS boundaries.
[Forks and long-running sessions](forks-and-compaction.md) traces copied versus
referenced history, inherited subagent prefixes, identity and compaction costs.

## Specification map and historical compatibility

The target is saved historical and present data. Writer behavior at one revision
is evidence about that revision, never an import allowlist. Start with
[historical formats](historical-formats.md) for unwrapped/bare records and earlier
enveloped families. Four concrete release snapshots are pinned there; their
differences are observations rather than guessed release ranges. New/unknown data
retains original fields/bytes until it can be interpreted.

| Question | Detailed specification | Checked inventory |
| --- | --- | --- |
| What wire shapes and payloads can occur? | [Wire format and codecs](wire-format.md) | [Record catalog](record-catalog.json): all 13 outer and 18 response variants at the main pin, nested declarations/codecs, compatibility shapes |
| How are data produced, transformed and filtered before writing? | [Production and persistence](production-and-persistence.md) | [Persistence matrix](persistence-matrix.json): all 86 protocol events, direct payload schemas, per-record import handling and tool preprocessing |
| How do readers, forks, compaction, migration and deletion work? | [Readers and lifecycle](readers-and-lifecycle.md) | [Lifecycle catalog](lifecycle-catalog.json): 30 operations, all 19 core and 19 public item variants, reader-specific backward handling |
| What files/tables exist and what do their rows mean? | [Storage surfaces](storage-surfaces.md) | [Storage schema](storage-schema.json): 27 final tables across six migration families |
| What did older source actually write and read? | [Historical formats](historical-formats.md) | [Historical snapshots](historical-snapshots.json): 0.20.0, 0.50.0, 0.100.0 and 0.140.0 declarations and writer/reader evidence |

The enum inventories are checked against Rust ASTs at the source pin. The SQLite
inventory is checked against public migrations replayed in fresh in-memory
databases. Source references, JSON catalogs and authored examples are also checked.
See [the recheck command](storage-surfaces.md#rechecking-this-snapshot). These checks
measure documentation coverage; upstream behavior tests were inspected as source.

Arbitrary provider/tool/plugin JSON remains explicitly open. Historical reader
decisions can normalize, skip or reject material that our archive still needs to
preserve. Known implementation gaps in llm-history are listed separately from
format coverage; documentation of a shape does not mean its semantic import is
already implemented.

## Evidence and compatibility

**Writer evidence** below means the pinned Rust writer, serialization and
persistence policy were inspected together. **Reader evidence** means a decoder,
projection or migration accepts a shape; that alone does not establish that a
current writer emits it. No personal transcript, state database, credentials or
live runtime was opened; upstream tests were read rather than executed.

The decisive distinction at this revision is `codex-rs/history` versus
`codex-rs/rollout`: history owns the durable item/envelope types; rollout owns
file serialization, discovery, compression and append policy. File consumers
should pin both. The app-server API has its own camelCase public projection.
It is a useful conversation reader but its emitted turn/item objects have a
different representation from stored JSONL. [History types][history],
[wire codec][wire], [canonical JSON parser][parser], [writer][writer],
[policy][policy], [public API documentation][api].

## Location, discovery and dependencies

| Artifact | Meaning and discovery rule | Evidence |
| --- | --- | --- |
| `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread_id>.jsonl` | Active durable rollout; default home is `~/.codex`. Date directories and filename use local creation date/time with a UTC fallback; record timestamps use UTC. Recursively inventory rather than derive a date from the thread ID. Older flat files also exist. | [Path writer][paths], [discovery][list] |
| `rollout-<timestamp>-<thread_id>_<rollout_id>.jsonl` | Replacement rollout after revert: stable thread identity and physical rollout identity differ. | [Filename type][filename], [revert writer][revert] |
| `$CODEX_HOME/archived_sessions/` | Archived rollouts; include them for durable-history coverage. Archive status is an availability/display property. | [archive implementation][archive] |
| `.jsonl.zst` sibling | Compressed representation of the decoded JSONL stream. A read must support either representation; append materializes plain JSONL. Do not count both siblings as two conversations. | [compression reader and paths][compression] |
| `$CODEX_HOME/session_index.jsonl` | Append-only names: `{id,thread_name,updated_at}`, newest name wins; an index rewrite can remove a deleted thread's names. Capture if titles matter. | [name index writer][names] |
| `state_5.sqlite` and `thread_history_1.sqlite` under configured `sqlite_home` | Current rollout-path pointer, thread discovery, archive state and paginated projections. `sqlite_home` defaults through `$CODEX_SQLITE_HOME` to `$CODEX_HOME`. A consistent current-thread inventory needs this pointer after revert. | [configuration][config], [revert cutover][revert], [store][store] |
| `$CODEX_HOME/history.jsonl` | Prompt recall: `{session_id,ts,text}`, `ts` is Unix seconds. It contains user prompt occurrences and has its own persistence/size policy. The full conversation comes from rollouts. | [prompt history][prompt] |

The pinned database filenames come from the [SQLite configuration][sqlite].
The history database projects `thread_turns`, `thread_items`,
`thread_history_projection_state` and `thread_realtime_items`; its checkpoint
tracks decoded JSONL offset and ordinal. These are derived views; complete
native rollouts retain additional fields. Use consistent read-only SQLite
snapshots if capturing current-pointer/projection evidence, including committed
WAL state when that journal mode is in use. [Projection code][projection].

The filename's IDs, metadata's IDs, and SQLite pointer have distinct roles.
A physical file can survive after its thread switches to another rollout. A
filesystem-only archive should retain it with its actual revision and source
path, and label selection of the current logical history unresolved until it
has current-pointer evidence. Selecting the lexicographically newest filename
is not the source's selection contract. [Revert][revert], [lineage][lineage].

Codex subagents are separate threads/rollouts in these same stores. They are
linked by `parent_thread_id`, session `source`, root `session_id`, agent fields
and inter-agent communications. A subagent may also contain inherited parent
context; `subagent_history_start_ordinal` separates that inherited prefix from
its own projected history. Delegation, fork and inherited context need separate
edges. [Session metadata][session], [lineage][lineage].

## Native identity, ordering and time

- `SessionMeta.id`: stable thread ID. `session_id`: root thread's ID shared by
  its family; older metadata lacking this field is decoded using `id`.
- `rollout_id`: physical revision identity, ordinarily equal to thread ID;
  revert creates a distinct value encoded in the filename.
- `HistoryPosition.thread_id`: **a rollout ID despite the historical field
  name**. Its `end_ordinal_exclusive` and `end_byte_offset` select an immutable
  prefix. Offsets address decoded JSONL bytes, including record separators.
- `forked_from_id`: logical fork parent; `forked_from_ordinal_exclusive` marks
  its logical inherited boundary. `history_base` is the physical prefix source;
  revert can change it while retaining logical ancestry.
- `turn_id`, `root_turn_id`, Responses `id`, `response_id`, `call_id`, context
  window UUIDs, and history sequence counters have distinct scopes. Preserve
  them as native values, including absent IDs.

These are explicit type and writer contracts, not UUID-pattern guesses.
[Metadata and HistoryPosition][session], [fork cutoff][fork], [revert][revert].

The outer line is `{timestamp,ordinal?,type,payload,...}`. Writer timestamps
are UTC ISO strings with three fractional digits, e.g.
`2026-11-07T19:00:00.123Z`. `SessionMeta.timestamp` is session creation time,
not each item's occurrence time. Nested
`internal_chat_message_metadata_passthrough.create_time` is fractional Unix
**seconds**, retained as a JSON number. Preserve decimal precision.
[Writer][writer], [response metadata][models].

Legacy rollouts omit ordinal. Paginated new rollouts start at zero or the
inherited prefix's exclusive ordinal, and advance after each successful record
write. The session-meta record itself occupies an ordinal. Ordinals identify
native record positions, including metadata/control records; they are not user
turn numbers. Resume derives the next value from durable records and checks
inherited subagent-prefix completeness. [Ordinal implementation][ordinal].

Use physical record order for a raw file and resolve bounded lineage segments
for a logical thread. Timestamp sorting can change accepted-input order:
`metadata.user_input_order` records acceptance/delivery order, while
`history_sequence` and `sampled_through_sequence` have context bookkeeping
meanings. Keep all three. [Harness metadata][history], [retained context][retained].

## Forks, compaction, rollback, archive and deletion

**Paginated forks:** prepare/flush the source, resolve its lineage, find the
requested turn boundary, and store a `history_base` prefix reference. The child
file can depend on ancestor files rather than contain complete copied history.
Forking through an in-progress turn is rejected. A capture must recursively
close these bounded dependencies, preserve their raw files/decoded record
positions and retain the prefix even if an upstream parent disappears later.
Legacy/copied inherited histories still occur. [Fork implementation][fork],
[lineage resolution][lineage], [metadata][session].

**Compaction:** `compacted` is a durable checkpoint. Its `message` may be a
summary, and `replacement_history` may contain complete model replay items,
including encrypted compaction blobs. Optional `replacement_history_metadata`
is a positional companion array; lengths must match. Keep `guardian_history`,
`retained_context`, MCP resource origins, window identities, usage checkpoint
and resume metadata when present. Compaction replacement is a model-context
transition, not a new occurrence of every original message in the replacement.
Raw earlier conversation and post-compaction effective context are separate
views. [Checkpoint codec][checkpoint], [model-context replay][initialization].

**Retained context:** bounded original user/assistant text and verified answers
can survive outside model summarization. Completeness and origin fields matter;
missing text with `complete:false` means evidence is unavailable. One family
currently retains at most eight records, 16 KiB per record and 64 KiB per family.
`accepted_file_audio` records accepted provider file references. Delivered
assistant messages have a compatibility encoding as response items with harness
markers; decode that wrapper before creating an additional conversation
occurrence. [Retained families][retained], [compatibility wire transform][wire].

**Revert:** an unloaded paginated thread gets a new immutable rollout with the
same stable thread ID and new physical rollout ID. It references the retained
prefix; SQLite atomically switches the current pointer. Old files remain intact.
An importer should record a new selected-history revision and withdrawn
visibility for excluded turns while retaining prior captured content according
to its own archive policy. [Revert writer][revert]. Legacy rollback events also
exist; legacy migration has rollback planners/replay. Preserve unfamiliar event
payloads before interpreting them. [Migration][migration].

**Archive/unarchive/delete:** archive moves files into the archive area;
unarchive restores an active path. Delete handles descendant threads and shared
rollout references. The delete implementation rejects external fork-history references and attached
children before removal; batch deletion permits references internal to that batch.
Capture relationships before removal and do not make disappearance during a
partial scan an instruction to discard already captured history.
[Archive][archive], [unarchive][unarchive], [delete][delete], [reference index][refs].

**Migration and retention:** the pinned source has an explicit legacy-to-paginated
migration state machine: stage canonical JSONL, project/verify SQLite, publish
atomically, and recover through `.pending` journals. File bytes, compression and
physical path may change while logical bodies remain reusable. Treat that as a
source revision boundary and reconcile rather than append from an old offset.
The prompt-history size cap is independent from rollout retention. This review
does not establish one universal age-based rollout retention setting across
CLI, desktop, remote hosts and account-managed stores. [Migration][migration],
[compression][compression], [prompt history][prompt].

## Verification and remaining questions

This documentation was checked against the cited type definitions, actual JSONL
writer, persistence filter, canonical reader, fork/revert implementations and
migration orchestration. Authored examples were JSON parsed locally. No Codex
build, model call or source-history scan was required.

Runtime deployment flags, host-specific storage remapping, remote history stores,
provider-side file retrieval/expiry, and completeness of material suppressed
before the writer remain deployment-specific. The policy intentionally omits
transient variants and truncates some event payloads; preserved rollout bytes
are the available durable record, not a guarantee of every byte ever produced
by a tool. [Policy][policy]. Re-review these pinned sources before broadening an
adapter's schema assumptions.

[history]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/lib.rs#L55-L405
[wire]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/rollout_payload.rs#L33-L311
[checkpoint]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/rollout_payload.rs#L324-L460
[parser]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/lib.rs#L47-L87
[writer]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/recorder.rs#L2064-L2110
[policy]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/policy.rs#L16-L116
[paths]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/recorder.rs#L1717-L1739
[filename]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/rollout_file_name.rs#L1-L87
[list]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/list_files.rs#L1-L180
[compression]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/compression.rs#L63-L110
[names]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/session_index.rs#L23-L109
[config]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/config/src/config_toml.rs#L371-L383
[store]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/mod.rs#L1-L120
[session]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L3440-L3623
[models]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/models.rs#L883-L1395
[ordinal]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/ordinal.rs#L16-L131
[fork]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/paginated_fork.rs#L18-L170
[lineage]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/rollout_lineage.rs#L279-L597
[revert]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/revert_thread.rs#L15-L151
[archive]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/archive_thread.rs#L1-L160
[unarchive]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/unarchive_thread.rs#L1-L150
[delete]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/delete_thread.rs#L1-L229
[refs]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/rollout_reference_index.rs#L1-L160
[retained]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/retained_context.rs#L26-L235
[initialization]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/model_context.rs#L27-L96
[migration]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/rollout_migration.rs#L1-L84
[prompt]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/message-history/src/lib.rs#L1-L105
[api]: https://learn.chatgpt.com/docs/app-server#api-overview

[sqlite]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/state/src/sqlite.rs#L48-L54
[projection]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/thread_history.rs#L26-L124

# Codex forks, subagents and long-running sessions

Last reviewed **2026-11-07** against `openai/codex`, branch `main`, checkout
**`2bc8e7f5459b084afa482c7aac3f56fef36286b3`**. A fresh read-only reviewer traced
local writers, readers, call sites and tests; the parent checked the key paths.
No personal histories/databases were inspected and upstream tests were not run.
These findings describe the pinned source, including conditional and legacy paths.

## Three inheritance paths

| Path | Persisted representation | Import consequence |
| --- | --- | --- |
| Legacy user fork | Copies selected source history into a fresh rollout | Keep fork evidence so inherited records are distinguishable from new activity. |
| Paginated user fork | Writes `history_base`, a bounded reference to physical ancestor history | Resolve and retain the exact ancestor prefix, including nested ancestry. |
| Forked subagent, including paginated | Copies filtered model context into the child rollout | Honor the inherited-prefix boundary before counting child messages/tools/usage. |

The user-fork paths explicitly choose copied or referenced persistence. Referenced
fork startup reconstructs context in memory, then removes the inherited item count
before persisting local settings and any synthesized interruption suffix.
[Fork preparation][user-fork], [prefix persistence][initialization].

Paginated preparation freezes ordinal and decoded-byte cutoffs in `HistoryPosition`.
A fork point inside inherited history can target an older ancestor rollout directly.
The source is reserved until the child's reference becomes durable.
[Bounded preparation][prepare], [reservation tests][reservation-tests].

## Forked subagents

Subagent preparation flushes parent persistence before acquiring context. It loads
legacy history or paginated latest model context, filters it, and returns copied
fork history. A child with no fork mode starts fresh.
[Preparation][agent-preparation], [spawn call site][spawn].

Ordinary filtering keeps selected messages and context/control records, while
removing reasoning, most tool calls/results, dedicated token usage records and
several other parent-only records. Filtering has exceptions, including id-less
function outputs and configuration items; use the actual record predicate rather
than a blanket role/type rule. Conditional multi-agent V2 `preserve_fork_prefix`
can preserve all response items. It supplies inherited-context notices and adjusts
message envelope metadata. The latest legacy compaction without replacement
history forces context-baseline rebuilding and disables that preservation path.
[Filter][filter], [V2 handling][v2], [checkpoint handling][child-checkpoints].

Copied compaction checkpoints have parent cumulative token usage, guardian history
and turn attribution cleared; their replacement histories are also filtered.
Under `Feature::TokenBudget`, forked child checkpoint window identities/numbers
are rewritten to the child's initial window. Keep native representation and its
inheritance evidence together. [Checkpoint transformation][child-checkpoints],
[child window reset][identity].

For a paginated child, `create_with_inherited_model_context` computes the number of
persisted inherited items and writes `subagent_history_start_ordinal = count + 1`
in the header before appending that context. Startup avoids appending it again.
Projection advances across ordinals below this boundary with an empty change set:
those records serve model reconstruction without appearing as new child history.
[Inherited writer][inherited-writer], [projection exclusion][projection],
[writer/projection tests][subagent-tests], [projection tests][projection-tests].

## Identity axes

| Field | Meaning |
| --- | --- |
| `session_id` | Root agent family/controller identity |
| `SessionMeta.id` | Logical thread/conversation identity |
| Physical `rollout_id` | Physical history identity; can change on revert |
| `forked_from_id` | Immediate logical history source |
| `parent_thread_id` | Execution/spawn parent |
| `history_base.thread_id` | Physical history dependency, despite the field name |

Ordinary forked subagents and descendants share their controller's root session
identity while keeping distinct thread IDs and immediate parent/fork identities.
A normal root user fork establishes a new family. Persisted resume identity and
an explicitly inherited session ID have precedence; ephemeral prompt-cache routing
is selected separately. Legacy missing `session_id` defaults to the thread ID,
and child resume can discard that synthesized value in favor of controller identity.
An isolated old child export can therefore have incomplete family evidence.
[Assignments][identity], [child spawn assignments][child-assignment],
[identity tests][identity-tests], [legacy metadata default][legacy-default].

## Long-running sessions and compaction

Compaction replaces in-memory model context and **appends** a durable checkpoint.
The writer clones replacement history into `CompactedItem`, alongside retained
context, usage and resume metadata. It then persists `Compacted`, full `WorldState`,
`TurnContext` and applied settings. The rollout file is append-opened; preceding
conversation/tool records and older checkpoints remain.
[Compaction installation][compaction], [append writer][append-writer].

Repeated compactions can repeat retained content within checkpoint payloads.
Forked subagents can copy those checkpoints and their effective context again.
These are concrete sources of repeated storage. Counting embedded replacement
items as new conversation/tool/usage occurrences would also inflate activity.
Normal resume reconstructs context and persists changed settings; it does not
append the whole loaded historical prefix again. [Resume/fork initialization][initialization].

Legacy reconstruction reads and parses the full rollout into a vector. Paginated
reconstruction reverse-scans bounded lineage segments, stopping at the newest
complete checkpoint with replacement history and a window number. An incomplete
latest checkpoint forces full replay, even when older checkpoints are complete.
[Legacy reader][legacy-reader], [lineage context reader][context-reader],
[checkpoint scan policy][scan-policy].

Plain JSONL supports seeking to the relevant tail. A compressed segment is fully
decompressed into an anonymous temporary file before seeking/reverse parsing.
Temporary disk and decompression I/O therefore scale with each opened physical
segment, including bytes beyond a requested inherited cutoff. Ancestor reads use
the same mechanism. Cancellation is checked during decompression. Prefix validation
has a separate frame-header optimization that does not eliminate this later full
decode. [Seekable reader][seekable], [ancestor reads][ancestor-reads].

Paginated revert creates a new physical rollout, retaining logical thread identity
and a bounded reference to the kept prefix. SQLite switches the selected rollout
path. Previous rollouts remain, and existing children retain their frozen physical
references. [Revert][revert].

The incremental original-record reader is restricted to paginated roots without
fork/parent linkage, `history_base`, an inherited subagent boundary or a changed
physical rollout ID. It is not a general incremental reader for all these shapes.
[Original-history restrictions][original-history].

## Consequences for llm-history

These are importer design implications, rather than an implemented schema contract:

- Retain physical source records once and represent bounded inheritance explicitly.
  Share extracted body bytes through CAS while preserving distinct occurrences.
- Keep logical fork, execution parent, family membership and physical dependency
  as separate relationships. One parent column cannot preserve all four meanings.
- Provide separate transcript/activity and effective model-context projections.
  Inherited prefixes and replacement checkpoints carry context without asserting
  another execution of the included conversation/tool/usage records.
- Track current physical rollout selection and retain superseded captured revisions.
  Capture referenced ancestor prefixes before claiming complete logical history.
- Scope ordinals/offset checkpoints to physical rollout identity. Offsets address
  decoded JSONL; compressed-file offsets have a different meaning.
- Preserve unknown fields and native inheritance/window metadata for future replay.
  Content equality alone is insufficient evidence that two occurrences are inherited.
- Reuse captured segments and derived content across queries. Keep acquisition,
  decompression and replay bounded instead of rebuilding every descendant history.

At this review, the Rust adapter retains raw records and session metadata and
interprets basic fork/spawn links. It does not yet resolve `history_base` or apply
`subagent_history_start_ordinal`; its current single parent/relation representation
does not capture these independent axes. The proposed content-CAS/graph schema is
still unimplemented. See [the ingestion/storage assessment](../../ingestion-storage.md).

## Synthetic scenarios

1. Root, reference-backed fork and fork from an inherited turn; append to each and
   verify frozen boundaries, shared content and local activity counts.
2. Filtered forked child, V2 preserved-prefix child and grandchildren; compare
   inherited visibility, tool/usage counts and transformed checkpoint metadata.
3. Compaction, fork, parent revert and resume; keep logical IDs stable and the
   child's original physical dependency resolvable.
4. Mid-turn fork; attribute the synthesized abort suffix to its child snapshot.
5. Interrupted child-prefix write; preserve the declared inherited boundary and
   report incomplete coverage instead of projecting inherited items as local work.
6. Compressed ancestor with a small referenced prefix; bound importer acquisition
   and verify decoded offsets independently of compressed size.

Hosted/CCA storage, full migration canonicalization and legacy rollback replay were
not exhaustively reviewed in this pass. These findings concern inspected local paths.

[user-fork]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/thread_manager.rs#L1745-L1892
[prepare]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/paginated_fork.rs#L18-L197
[initialization]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/session/mod.rs#L1782-L1900
[reservation-tests]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/thread_history_materialization_tests.rs#L1425-L1459
[agent-preparation]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/agent/history.rs#L169-L290
[spawn]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/agent/control/spawn.rs#L734-L815
[filter]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/agent/history.rs#L38-L104
[v2]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/agent/history.rs#L290-L348
[child-checkpoints]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/agent/history.rs#L415-L482
[inherited-writer]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/live_thread.rs#L152-L180
[projection]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/thread_history_materialization.rs#L278-L285
[subagent-tests]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/agent/control_tests.rs#L2491-L2592
[projection-tests]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/thread_history_materialization_tests.rs#L1462
[identity]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/session/session.rs#L1055-L1112
[child-assignment]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/thread_manager.rs#L2250-L2272
[identity-tests]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/session/tests.rs#L7427-L7558
[legacy-default]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L3609-L3615
[compaction]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/core/src/session/mod.rs#L4864-L4926
[append-writer]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/recorder.rs#L1742-L1760
[legacy-reader]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/recorder.rs#L1129-L1178
[context-reader]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/model_context.rs#L186-L214
[scan-policy]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/model_context.rs#L18-L48
[seekable]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/seekable_reader.rs#L68-L106
[ancestor-reads]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/rollout_lineage/ancestor_reads.rs#L62-L86
[revert]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/revert_thread.rs#L116-L200
[original-history]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/original_history.rs#L33-L68

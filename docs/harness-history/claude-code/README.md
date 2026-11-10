# Claude Code durable conversation history

Reviewed **2026-11-07** using these independently versioned sources:

| Source | Exact reviewed revision | Version evidence |
| --- | --- | --- |
| `anthropics/claude-code`, `main` | **`0ed94698aa0aeee471465fc9c867c2e8434c4d68`** | Public changelog starts at **2.1.319**; this does not identify a locally installed executable. |
| `anthropics/claude-agent-sdk-python`, `main` | **`2209f2c134fee914c0f75733b631a5fc238ca846`** | `pyproject.toml` declares **0.2.179**. |
| Official Claude Code documentation | Live pages reviewed on the date above | Documentation can change independently of both commits. |

[Code changelog][changelog], [SDK version declaration][version].

The Claude Code runtime/transcript writer is closed source. The public Code
repository supplies release notes and integrations; the SDK supplies substantial
open reader/mirror/mutation code. This distinction matters for a storage parser:
the SDK deliberately types entries as an open structural supertype and describes
the actual discriminated transcript union as internal. Preserve original JSON
before projecting the fields described here. [SDK entry contract][store-types],
[official storage description][sessions-doc].

[Stored format and reader behavior](format.md) covers native fields, branching,
parallel tools and compaction. [Acquisition and CAS implications](ingestion.md)
covers dependency capture, source coverage and reusable bodies. Examples are
newly authored synthetic fragments in [examples/](examples/).

## Evidence levels

- **Published contract:** official storage/session documentation, reviewed on
  the date above. Strong evidence for supported locations and user-visible
  behavior; it explicitly allows the internal entry format to change.
- **SDK implementation:** exact pinned filesystem reader, mirror interface and
  public mutation writers. Strong evidence for what these functions accept,
  return and write; it does not expose every internal CLI writer branch.
- **SDK tests:** authored fixtures and an end-to-end parallel-tool test support
  reader behavior. Tests were inspected, not executed. An e2e test's expectations
  are useful source evidence, not a runtime result from this review.
- **Inference/unknown:** explicitly labeled where dependencies or writer fields
  are broader than available public implementation. No private transcript,
  account data, live Code runtime or database was inspected.

## Filesystem storage and discovery

| Artifact | Purpose and coverage | Evidence |
| --- | --- | --- |
| `~/.claude/projects/<project>/<session-id>.jsonl` | Durable main transcript: one JSON object per line. `$CLAUDE_CONFIG_DIR` replaces `~/.claude`. | [Published location][sessions-doc], [SDK root resolver][paths] |
| `<session-id>.orphaned-<timestamp>-<suffix>.jsonl` and `<session-id>.jsonl.superseded-<timestamp>` | Previous transcript revisions set aside by Code; excluded from the picker. Capture separately as historical revisions, retaining filenames. | [Published application data][directory-doc] |
| `<project>/<session-id>/subagents/**/agent-<agent-id>.jsonl` | Child conversation transcripts; SDK recursively discovers nested workflow run directories as well as direct children. | [SDK subagent discovery][subagents] |
| `agent-<agent-id>.meta.json` beside child transcript | Optional child metadata, including `toolUseId` and `parentAgentId`; links spawning tool call and parent agent. | [SDK metadata reader][sidecars] |
| `<project>/<session-id>/tool-results/` | Referenced large tool outputs; preserve actual pointers and available bytes. | [Application data][directory-doc]; per-tool pointer schemas remain internal |
| `$CLAUDE_CONFIG_DIR/uploads/` (default `~/.claude/uploads/`) | Remote Control path-based attachments. | [Application data][directory-doc] |
| Code temp tree (platform-dependent; configurable with `CLAUDE_CODE_TMPDIR`) | Pasted/attached images and session scratchpad artifacts; resolve actual recorded paths. | [Application data][directory-doc] |
| `history.jsonl` | Prompt recall/UI history. Full conversation acquisition uses project transcripts. | [Application data][directory-doc] |
| `file-history/<session-id>/` and related checkpoints | Undo/file restoration dependencies; capture if promising historical file restoration, in addition to conversation history. | [Published checkpoint distinction][sdk-sessions] |

Default `<project>` is the working-directory path with every non-alphanumeric
character replaced by `-`. Long converted names are truncated to 200 characters
and receive a hash suffix. Official docs also publish
`CLAUDE_CODE_PROJECT_DIR_NAME` to choose that directory name **when
`CLAUDE_CONFIG_DIR` is also set**. It is read at startup and accepts 1–64
letters/digits/underscores/hyphens and excludes Windows device names; invalid values are ignored.
[Custom-name conditions](https://code.claude.com/docs/en/sessions#name-the-project-directory-yourself).
**Reader discrepancy:**
the pinned Python helper derives a sanitized cwd, includes realpath/worktree
fallbacks, and tolerates long-path hash differences; its root resolver in
`sessions.py` does not consult this custom-name variable. Use a filesystem
inventory or an explicitly resolved `transcript_path` rather than assuming
`get_session_messages(directory=...)` discovers every custom-layout transcript.
[Published path behavior][sessions-doc], [SDK sanitizer/resolver][paths],
[SDK lookup][lookup].

The SDK lookup validates main session IDs as UUIDs, looks for
`<id>.jsonl`, and searches project directories if no directory is supplied.
Its lite list reads stat plus 64 KiB head/tail buffers; it can deduplicate by
session ID and skip unnamed/sidechain entries. A UI listing is therefore a
projection rather than exhaustive file coverage. Preserve the source project
key and scope every native lookup by profile/store. [Lite listing][listing],
[lookup][lookup]. No filesystem path can be reconstructed reliably by reversing
the sanitized name alone.

## Retention, revisions and export

**Published CLI retention:** current docs give a 30-day default, minimum one day,
for `cleanupPeriodDays`; zero is invalid. Main/child transcripts and set-aside
revisions participate in cleanup. `CLAUDE_CODE_SKIP_PROMPT_HISTORY` suppresses
transcript and prompt-history writes; non-interactive runs also support
`--no-session-persistence`. [Application data][directory-doc]. Capture coverage
must include whether persistence was enabled and whether dependent files still
exist; an absent transcript can reflect configured suppression or cleanup. Desktop/Cowork transcripts have
separate age exemptions and `desktopSessionCleanupPeriodDays`; managed retention
and HIPAA configuration can apply the regular cutoff. [Retention details][directory-doc].

**Published interfaces:** `/export` produces a readable export. Non-interactive
JSON/stream-json outputs report run/result/session/usage information; hooks and
status-line inputs expose `transcript_path`. SDK historical helpers return
selected user/assistant messages. These are different artifacts with distinct
fidelity. [Session interfaces][sessions-doc], [SDK projection][projection]. An
export does not carry a guaranteed reversible encoding of all internal records.

**SDK mutations:** rename and tag append metadata; delete unlinks main JSONL
and removes the sibling session directory; fork creates another file. These
public SDK writers are directly reviewable. They do not establish the complete
CLI implementation for rewind, compaction, physical retention or repair.
[Fork/delete/metadata writers][mutations]. Keep both source revisions and the
currently selected conversation when collecting set-aside files.

**Rewind/compaction:** source messages can remain in the raw transcript outside
the currently selected chain. Compaction's `logicalParentUuid` backpointer links
earlier history but the SDK deliberately excludes it from effective-context
traversal. The `isCompactSummary` user message replaces earlier context in that
projection and is retained. [SDK chain builder][chain]. Preserve the old records
and the compaction edge so full captured conversation and effective resumed
context can both be represented.

## External session stores and runtime output

The SDK `SessionStore` mirrors JSON-safe stored entries after successful local
writes. Its keys are `project_key`, `session_id`, optional `subpath`; a child
can use `subagents/agent-<id>` or nested workflow paths. `append` and `load` are
required; listing, summary listing, deletion and subkey listing are optional.
Returned objects must be deeply equal, while original key order/whitespace need
not survive. The store owns its retention; local cleanup runs independently.
[SDK store contract][store-types], [published external storage][storage-doc].

The Python read loop intercepts `transcript_mirror` frames, queues their entries
for the store, and keeps them out of ordinary application message iteration.
Near-real-time frames and buffered flushes have different durability times;
mirror failures can drop batches after retries. A mirror inventory is therefore
only as complete as its recorded flush/error coverage and adapter behavior.
[Read loop][mirror-loop], [store contract][store-types].

A subagent `.meta.json` is represented in a store stream as synthetic
`{type:"agent_metadata",...}` entries; the last such entry wins. Preserve that
representation separately from on-disk child JSONL and don't interpret it as a
new conversation message. Summary sidecars contain opaque SDK-owned data and
storage-write `mtime` in Unix milliseconds; preserve rather than reinterpret
that state. [Sidecar conversion][sidecars], [store types][store-types].

`SessionMessage` uses Python `session_id`, `parent_tool_use_id`,
`parent_agent_id`; stored native entries use camelCase fields such as
`sessionId` and `parentUuid`. Live `AssistantMessage` has convenience fields
including `message_id`, `usage` and `stop_reason`. These types are projections;
serializing one cannot reconstruct all stored envelope fields or provider blocks.
[Historical result type][message-types], [projection][projection],
[live types][live-types].

## Validation and unresolved writer details

This review inspected the filesystem reader, branch selection, split-block and
parallel-result recovery, subagent sidecars, mirror types/read-loop handling,
mutation writers, unit fixtures and e2e expectations. The synthetic examples
were parsed as JSON locally. An isolated extraction of the pinned pure Python
reader functions also recovered both parallel calls/results exactly once across
seven visible synthetic entries; no dependency imports or runtime/model calls
were involved. Upstream test suites were not executed.

The complete CLI entry union, request-record schemas, atomic append/flush
mechanics, exact compaction/rewind rewrite conditions, provider-specific opaque
payload interpretation and all migration rules remain internal. The public
changelog documents transcript-ordering, cleanup and truncation bug fixes,
which supports recording native version and preserving unknown records at
capture. It does not supply a complete writer specification. [Changelog][changelog],
[internal-entry contract][store-types].

[changelog]: https://github.com/anthropics/claude-code/blob/0ed94698aa0aeee471465fc9c867c2e8434c4d68/CHANGELOG.md#L1-L28
[version]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/pyproject.toml#L1-L12
[paths]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L81-L190
[lookup]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L836-L895
[listing]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L353-L737
[chain]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L897-L1262
[subagents]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1427-L1512
[sidecars]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1643-L1710
[projection]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1248-L1340
[store-types]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1881-L2057
[message-types]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2109-L2180
[live-types]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1205-L1430
[mirror-loop]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/query.py#L450-L490
[mutations]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/session_mutations.py#L53-L479
[sessions-doc]: https://code.claude.com/docs/en/sessions#where-transcripts-are-stored
[directory-doc]: https://code.claude.com/docs/en/claude-directory#application-data
[sdk-sessions]: https://code.claude.com/docs/en/agent-sdk/sessions
[storage-doc]: https://code.claude.com/docs/en/agent-sdk/session-storage

# Claude Code capture and content-addressing implications

Reviewed **2026-11-07** against Python SDK `main`
`2209f2c134fee914c0f75733b631a5fc238ca846` (0.2.179), Code public repository
`main` `0ed94698aa0aeee471465fc9c867c2e8434c4d68` (changelog 2.1.319), and
[official storage documentation](https://code.claude.com/docs/en/sessions#where-transcripts-are-stored).
These are proposed llm-history-rs adapter rules derived from the source review;
they are not changes to the closed Claude Code writer.

## Acquisition scope and incremental capture

Inventory the effective config root and every applicable project directory,
including custom names. Include the main JSONL, set-aside orphaned/superseded
revisions, nested child JSONL and child metadata sidecars. Persist source profile,
project key, session ID, child subpath, exact source artifact path/revision and
coverage. A session picker/helper listing can omit files; its absence is not a
complete source inventory. [Published location][published], [SDK lookup][lookup].

Capture complete lines from a bounded read and retain all object variants,
including request/control metadata outside the historical helper's chain-entry
allowlist. Preserve incomplete-tail/error coverage instead of silently adopting
the helper's skip-corrupt-lines behavior as an archival promise. Record file
identity, size, mtime and complete-record checkpoint; replacements, set-aside
files and source truncation require revision reconciliation rather than a blind
append from the previous offset. This is an acquisition recommendation based
on [the permissive source reader][chain] and published set-aside revisions.

Store-backed acquisition needs the adapter's transcript key, append order,
subkeys, storage-write mtime and mirror error/flush coverage. Session UUID alone
is not a globally unique archive key across tenants, config roots or backend
instances. The SDK says to preserve append-call order within a process, and
storage-commit order across processes. The optional summaries' data is opaque;
its mtime must use the backend's storage clock. Keep the backend namespace in
native-ID resolution and checkpoint identity. [Store contract][store].

The mirror contract recommends UUID-based idempotency and append-without-dedup
for metadata lacking a UUID. Raw captures should retain every observed source
revision/batch occurrence for diagnosis, while an adapter's native projection
can reconcile UUID duplicates according to a documented policy. UUID reuse does
not imply identical content; preserve revisions if the stored object changes.
The in-memory reference implementation appends lists directly, so actual backend
idempotency behavior must be recorded rather than assumed from the interface.
[Store contract][store], [reference adapter][reference-store].

## Dependencies and availability

Capture dependencies explicitly referenced by the stored content:

- Inline text and media blocks: preserve source bytes/JSON and optionally decode
  base64 to shared binary content with MIME/encoding/position retained.
- Large tool results or attached media referenced by path: capture the referenced
  artifact with source/path, size, availability and content references. An absent
  result file remains an unresolved dependency; the displayed preview is only
  the available preview body.
- Child `agent-*.jsonl` and optional `.meta.json`: retain both, including nested
  workflow directories. Resolve `toolUseId` to the parent tool call and
  `parentAgentId` to a child writer where possible.
- Model-request records: retain actual system/instruction/tool-definition/request
  bodies where emitted. The reviewed SDK does not define their complete shape;
  unknown records stay decodable raw native objects.
- File checkpoints, loaded instruction files or other workspace artifacts:
  capture historical bytes only when promising replay or file restoration.
  Record their relationship to the transcript and distinguish a current file
  read from proven bytes used during the original request.

Source evidence: [child discovery/sidecars][children],
[published application data][directory], [checkpoint scope][sdk-sessions]. The
specific closure algorithm is an inference/recommendation because the closed
writer's reference schemas vary by tool and release. Preserve opaque provider
blobs/signatures and original associations even if local replay is unavailable.

## Content CAS versus native occurrences

| Native component | Reusable body | Occurrence/relationship metadata kept separately |
| --- | --- | --- |
| User/assistant string content | Exact text bytes | Store/project/session/agent, entry UUID, API message ID, parent, role, time, flags and provenance |
| Content list | Individual bodies plus versioned ordered-list structure | Block indices/kinds, per-block provider attributes, original string-vs-list representation |
| `tool_use` | Input object under a declared canonical-JSON policy, or exact source representation | Tool name, tool-use ID, assistant UUID/response ID, call position and agent scope |
| `tool_result` | Exact output string/ordered block content and media dependencies | Native result UUID, `tool_use_id`, `is_error`, source assistant, completion position and truncation/availability |
| Thinking/provider material | Thinking text and signature/opaque bytes as associated bodies | Block kind, provider/model context, replay coupling and original fragment position |
| Compaction summary | Exact summary text/blocks | `isCompactSummary`, compaction boundary, logical backpointer and selected-history revision |
| Copied fork entry | Reuse original logical content when equal | New UUID/session occurrence, `forkedFrom`, remapped parent and timestamp revision |
| Request/instruction/tool catalog | Structured bodies under explicit versioned encoding | Effective context position, source scope and relation to request or turn |
| Usage/cost/stop metadata | Native occurrence data | Response grouping, fragment relation and raw provider field names/scopes |
| Titles/tags/sidecars | Text/structured data where useful | Append occurrence, last-wins effective value, sidecar version and native key |

A raw line hash captures a native **observation** containing IDs, time and
wrapper data. It is useful for evidence preservation and byte reconstruction;
it does not deduplicate repeated semantic bodies. Extract body references after
capture and retain raw records for decoder upgrades. Two equal results from two
tool executions share body storage while retaining two call/result occurrences.
A split assistant response retains several entry UUIDs even when its fragments
share `message.id`. [Native entry contract][store], [parallel recovery][recovery].

JSON equality needs an explicit, versioned canonicalization policy. Preserve
absence/null/empty distinctions and array order. Keep exact text byte content
without trimming or Unicode normalization unless the format-specific decoder
explicitly establishes that transformation. Original JSON formatting and logical
JSON reconstruction are separate fidelity guarantees; SessionStore only promises
deep-equal objects, so exact original line bytes cannot be recovered from every
backend. [Store load contract][store].

## Graph/projection behavior

Keep distinct edge types for physical record containment/order, native
`parentUuid`, logical compaction ancestry, assistant response grouping, tool-use
result linkage, fork provenance and delegation. Preserve unresolved references
rather than guessing from proximity. The selected visible main conversation is
the SDK's chain plus parallel recovery and filtering; raw branches/metadata
remain another retained view. [Reader algorithm][chain], [parallel recovery][recovery].

Recovery order is not global file order or global time order. The reader chooses
its main leaf by file position and then inserts missing response siblings/results
under constrained ownership rules. The subagent reader uses a simpler last-leaf
walk. Capture enough native structure to recompute each projection when upstream
changes its reader. [Child reader][children].

When source files disappear after cleanup/delete, record upstream availability
separately from retained content. Rewind/fork/compaction can change selected
context without erasing every old raw record. Only a successful complete scan
of its declared scope can support marking a previously observed source absent;
a failed scan should retain the previous checkpoint and committed observations.
These are archive-design recommendations consistent with the source behaviors.

## Suggested adapter fixtures

- Split assistant tool calls sharing `message.id`, results parented to separate
  call UUIDs, and a later turn following only one branch.
- Tool result whose parent lies outside the response but has a same-agent source
  reference and answers an open unambiguous call; other-agent and duplicate
  results remain excluded from the recovered visible view.
- Compaction boundary with `logicalParentUuid` plus a retained compact summary;
  original captured text remains available while effective context follows the
  post-compaction chain.
- Identical bodies across a UUID-remapping fork: reuse body CAS, retain distinct
  occurrences and `forkedFrom` edges.
- Meta entries without UUID/timestamp, late title/tag clears and duplicate UUID
  revisions. Retain raw metadata and its effective last-wins projection.
- Long-path hash mismatch, custom project name, nested child workflow and missing
  `.meta.json`; discovery coverage and unresolved delegation remain explicit.
- Mirror retry/drop and backend JSON-key reordering; deep-equal body preservation
  remains distinguishable from byte-exact file capture.
- Superseded/orphaned files, partial final line and cleanup of a referenced tool
  artifact; capture survives with accurate coverage/availability.

[published]: https://code.claude.com/docs/en/sessions#where-transcripts-are-stored
[directory]: https://code.claude.com/docs/en/claude-directory#application-data
[sdk-sessions]: https://code.claude.com/docs/en/agent-sdk/sessions
[lookup]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L836-L895
[chain]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L897-L1022
[recovery]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1025-L1245
[children]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1427-L1738
[store]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1881-L2057
[reference-store]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/session_store.py#L66-L137

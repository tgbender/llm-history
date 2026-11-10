# Claude Code stored JSON and SDK reconstruction

Reviewed **2026-11-07**. Open reader/mutation source: Python SDK `main`
`2209f2c134fee914c0f75733b631a5fc238ca846` (0.2.179). Public Code release
notes: `0ed94698aa0aeee471465fc9c867c2e8434c4d68` (latest changelog heading
2.1.319). The closed runtime's complete writer union remains unpublished.

## Entry envelope and native field spelling

A stored JSONL file is a sequence of JSON objects. The [SDK store supertype][types]
requires only a string `type`; it optionally advertises `uuid` and `timestamp`,
with every additional field passed through opaquely. Some metadata entries have
neither ID nor timestamp. The following is an evidence-based inventory, not a
closed schema suitable for rejecting new fields.

| Field | Meaning / evidence boundary |
| --- | --- |
| `type` | Native discriminator, e.g. `user`, `assistant`, `system`, `progress`, `attachment`; also metadata/request/control kinds outside the conversation reader's allowlist. |
| `uuid` | Transcript-entry identity, used for parent lookup and store idempotency. The SDK parser requires a string for chain entries; it does not infer it from an API message ID. |
| `parentUuid` | Parent transcript-entry ID, nullable/missing at root; can target progress/system entries as intermediate chain links. |
| `logicalParentUuid` | Compaction backpointer; the main reader deliberately does not follow it for effective context. |
| `sessionId` | Native session UUID; projected to Python `session_id` by historical helpers. |
| `timestamp` | Native ISO timestamp string where supplied. SDK fork writer emits UTC ISO strings; it retains original times except the last copied message. Not every metadata append supplies a timestamp. |
| `message` | Raw API-style message object including `role`, `content`, and provider fields when present. The historical helper returns this dict intact. |
| `message.id` | API assistant response identity; several separate transcript entries/content blocks can share it. Distinct from `uuid`. |
| `isSidechain`, `agentId` | Writer/agent scope used when selecting main leaves and recovering tool results. |
| `isMeta`, `teamName` | Main conversation visibility and leaf-selection filters; keep their records even if a display omits them. |
| `isCompactSummary` | Compacted-context summary marker; intentionally included in SDK visible results. |
| `sourceToolAssistantUUID` | Assistant-entry reference assisting tool-result recovery; not interchangeable with a tool-use ID. |
| `forkedFrom` | SDK fork writer stamps `{sessionId,messageUuid}` for every copied entry. |
| `cwd`, `gitBranch`, `customTitle`, `summary`, `aiTitle`, `tag` | Metadata fields inspected by list/summary readers or produced by mutations; positions and discriminator vary. |
| Additional fields | Preserve version, request IDs, usage/stop metadata, subtype, origin and opaque provider data when present. Their full CLI-population contract is unresolved. |

[Parser/chain field use][chain], [projection][projection], [fork writer][fork],
[metadata reader][listing]. Stored fields use mixed spelling (`parentUuid`,
`sourceToolAssistantUUID`, `toolUseId`), while API/tool block fields usually use
snake_case (`tool_use_id`, `is_error`). Never apply a global casing conversion.

`SDKSessionInfo.last_modified` and `created_at` are **Unix milliseconds**;
filesystem mtime is multiplied by 1000, while created time is parsed from native
ISO timestamp. Store listing/summary `mtime` is also **Unix milliseconds**, but
means storage-write time, not the maximum message timestamp. Record native ISO
text, parsed time and observation/storage time separately. A copied transcript
can preserve old message times and have a new mtime.
[Stat reader][stat], [result types][message-types], [store types][types].

## Entry families and visibility

| Family | Evidence and preservation rule |
| --- | --- |
| `user` | A human/host input or tool result. Role alone cannot establish human authorship. Preserve full content and origin when present. |
| `assistant` | Assistant content block/response fragment; several entries may share one API message ID. Keep each entry UUID and ordered content blocks. |
| `progress`, `system`, `attachment` | Retained as graph-chain links by SDK parser, then filtered from visible `SessionMessage` output. `system` can carry compaction boundaries. |
| `summary` | Metadata outside the chain-entry allowlist. A title summary is distinct from a `user` message marked `isCompactSummary`. |
| `custom-title` | SDK rename writes `{type:"custom-title",customTitle,sessionId}`. |
| `tag` | SDK tag writes `{type:"tag",tag,sessionId}`; clearing writes empty string. |
| `content-replacement` | SDK fork parser preserves matching-session `replacements` arrays and emits a new record under the fork's ID. Exact replacement semantics remain runtime-owned. |
| Request records | Official docs state that transcript records can describe actual model requests, with system prompt/tool definitions/parameters. Their full discriminator/field schema is internal; capture unknown records rather than inventing an API-request schema. |
| `agent_metadata` | Synthetic external-store representation of a child `.meta.json` sidecar; last metadata entry wins. It is not an on-disk message variant inferred from the live API stream. |
| Other metadata/control entries | Keep full JSON even if the historical display reader skips them. Future decoders can revisit the raw observation. |

[Chain parser][chain], [metadata mutation writers][mutations],
[replacement preservation][replacement], [official transcript contract][storage-doc],
[sidecar handling][sidecars]. The internal union is intentionally open;
`queue-operation`, snapshots or other new records must not be classified as
utterances merely because they carry a UUID or text.

## Mixed content, tools, usage and opaque data

Historical `SessionMessage.message` is `Any`/raw JSON. Its content may be a
plain string or an ordered block list; the reader does not normalize it to
live Python dataclasses. Preserve string versus list, block boundaries, null,
empty arrays, nested media and unknown block fields. [Projection][projection],
[historical type][message-types].

The SDK [live content types][content-types] establish these common shapes and
semantics, while stored data remains the raw dict:

| Native/API block | Content and identity |
| --- | --- |
| `text` | `text` string, potentially additional raw provider fields. |
| `thinking` | `thinking` string and opaque `signature`; signature must stay associated with the block and model/provider context. |
| `tool_use` | `id`, `name`, `input` object. Preserve JSON structure and optional unknown provider fields. |
| `tool_result` | `tool_use_id`, `content` as string/list/null, optional `is_error`; content may itself contain text/media blocks. Typically appears within a `user` transcript entry. |
| Server-side tool use/result | SDK has typed use blocks (`id`,`name`,`input`) and result blocks (`tool_use_id`, opaque result dict). Results are provider-executed; retain actual native discriminator and fields. |
| Other media/provider blocks | Raw images/documents, opaque redacted/encrypted thinking or unfamiliar extensions can survive in historical dicts. Exact Code writer population is unverified here. Capture original JSON and referenced binary dependencies without converting it to a text-only live type. |

Link results by tool-use ID in session/agent scope; envelope `uuid`,
`sourceToolAssistantUUID`, `message.id` and `parentUuid` provide other
relationships. A parallel response can contain multiple tool calls and branch
into independently parented results. Tool-result content equality does not make
two executions the same occurrence. [Recovery implementation][recovery].

Usage is occurrence data. The live `AssistantMessage.usage` is a provider dict;
raw historical `message` may preserve its usage and stop fields. Avoid inferring
token units or cumulative scope from similarly named unknown fields. Split
assistant records may repeat response-level metadata: summing usage on every
fragment can overcount one response. Group by native response identity and keep
fragment-level evidence before defining a versioned accounting rule.
[Live assistant type][live-types], [split-block fixture][parallel-test]. This
accounting recommendation is an inference; the public reader does not compute
usage totals.

## The actual main-session reader algorithm

The pinned [reader][chain] does more than a generic tree walk:

1. Parse each JSONL object; keep only `user`, `assistant`, `progress`, `system`,
   `attachment` with string `uuid` for chain construction. Corrupt/non-object
   lines are skipped by this convenience helper. A capture importer should
   retain/report these instead of treating helper output as complete raw coverage.
2. Index by UUID (last value wins for duplicates) and remember file positions.
   Identify terminal entries not referenced as another entry's `parentUuid`.
3. Walk backward from each terminal until a user/assistant candidate leaf;
   stop cycles. Prefer candidate leaves without sidechain, team or meta flags.
   Choose the latest candidate **by file position**, not largest timestamp.
4. Follow `parentUuid` back to root with cycle protection and reverse the chain.
   Deliberately omit `logicalParentUuid`; following it would reintroduce
   pre-compaction content alongside the replacement summary.
5. Recover split assistant siblings and orphaned parallel tool results using
   the response grouping described below.
6. Keep visible `user`/`assistant` entries, exclude meta/sidechain/team entries,
   retain compact summaries, then apply offset/limit and map to `SessionMessage`.

`get_session_messages` returns the selected visible conversation. A raw file
capture can retain abandoned branches and hidden metadata alongside it. Maintain
both views without treating every off-chain entry as an abandoned tool result.
[Visibility and projection][projection].

### Split assistant blocks and parallel results

The recovery code's own comment documents a runtime pattern: one assistant entry
per content block, consecutive assistant entries sharing `message.id`, then tool
results parented to each call's own assistant UUID. A root-to-leaf walk can retain
one result and lose later calls from the same response. The SDK explicitly
repairs this shape. [Recovery code][recovery], [unit fixture][parallel-test],
[e2e test expectation][e2e]. These sources establish the reader's intended
behavior; the closed writer itself was not inspected.

The [implementation][recovery]:

- Anchors each on-chain response at its last on-chain assistant fragment and
  groups all assistant entries sharing that response ID.
- Builds call ownership from `tool_use.id`; a call ID claimed by different
  responses is ambiguous and cannot place an otherwise stray result.
- Indexes a result by its parent assistant, `sourceToolAssistantUUID`, and
  tool-use owner. Source/call-based association requires the same writer
  (`isSidechain` boolean and `agentId`).
- Recovers off-chain siblings and directly parented results; a result whose
  parent lies outside the response is recovered only if it answers a still-open
  unambiguous call. Already answered calls and other-agent strays are excluded.
- Orders recovered siblings and results separately by timestamp, with stable
  tie handling; inserts siblings then results after the response anchor. Original
  on-chain entries retain their chain order. This is not a global timestamp sort.

Preserve file order, graph order and projected recovery order as distinct
relationships. A single total timestamp sort loses the original selection and
response grouping evidence.

## Compaction, subagents and forks

`system` compact-boundary entries can carry `logicalParentUuid`; the selected
post-compaction chain relies on the compact summary rather than traversing that
backpointer. Keep the backpointer as a compaction/history relationship for full
archive reconstruction. Summary content and original captured text are separate
bodies; a summary cannot reconstruct missing original text. [Chain comment][chain].

Subagent discovery recursively finds `agent-*.jsonl` under the session's
`subagents` tree. Its pinned reader uses the **last user/assistant file entry**
as leaf, then follows `parentUuid` with cycle protection. This is a separate,
simpler algorithm, with an implementation comment assuming linear children
without compaction/preserved segments. It does **not** call the main-session
parallel recovery function. Keep raw child files so future/newer child shapes
can be interpreted independently. [Subagent algorithm][subagents].

An optional sidecar's `toolUseId` becomes `parent_tool_use_id`; `parentAgentId`
becomes `parent_agent_id`. Missing/unusable optional metadata leaves links
unknown. The filesystem sidecar is a JSON object, while a store stream carries
it as a synthetic typed entry. [Sidecar handling][sidecars].

The open [SDK fork writer][fork] copies the eligible prefix in original file
order, filters sidechains, remaps every copied entry UUID, walks through removed
progress ancestors to preserve parent links, remaps `logicalParentUuid`, stamps
the new `sessionId` and `forkedFrom`, clears session-specific team/agent/slug/source
fields, preserves earlier timestamps, and refreshes only the last writable entry's
time. It appends combined matching content replacements and a title. The source
file remains. This is exact behavior of `fork_session` / its shared transform;
it is not proof of every CLI `--fork-session` or interactive rewind path.

## Synthetic examples

- [parallel-tools.jsonl](examples/parallel-tools.jsonl): two assistant fragments
  share one API message ID; one parent walk omits the other result; recovery
  restores the response group. Includes mixed text/image tool-result content,
  opaque thinking signature, raw usage and late metadata.
- [compaction-and-fork.jsonl](examples/compaction-and-fork.jsonl): independent
  fragments illustrating compact boundary/summary and forked occurrence identity.
- [agent.meta.json](examples/agent.meta.json): optional child sidecar, explicitly
  synthetic, with spawn call and parent-agent IDs.

All examples are authored fragments with fake IDs/content. Image bytes/signature
strings are placeholders, not valid provider replay material. JSON syntax was
validated. The pinned pure reader functions were extracted and run against the
parallel example: all seven visible entries were recovered, including both calls
and both results exactly once. No runtime writer or model was invoked.

[types]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1881-L2057
[chain]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L825-L1022
[recovery]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1025-L1245
[projection]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1248-L1365
[listing]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L421-L518
[stat]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L353-L384
[message-types]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2109-L2180
[live-types]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1373-L1404
[content-types]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L1205-L1285
[mutations]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/session_mutations.py#L53-L180
[fork]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/session_mutations.py#L348-L479
[replacement]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/session_mutations.py#L590-L621
[subagents]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1427-L1512
[sidecars]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/_internal/sessions.py#L1643-L1710
[parallel-test]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/tests/test_sessions.py#L676-L715
[e2e]: https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/e2e-tests/test_parallel_tool_session_reads.py#L54-L148
[storage-doc]: https://code.claude.com/docs/en/sessions#where-transcripts-are-stored

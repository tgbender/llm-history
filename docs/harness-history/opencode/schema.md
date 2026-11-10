# OpenCode persisted schema

Last reviewed: **2026-11-07** at `dev` commit
`c33eaa170ed5093c08f1684862a695aae036dcda` (root version `2.2.5`).
Links below refer to that exact tree.

## Encoding and identity rules

SQL `text({mode: "json"})` columns contain JSON text; examples below show their
decoded JSON values for readability. SQL NULL and an absent JSON property are
different observations. Native optional JSON fields normally omit `undefined`
during encoding; optional does not add JSON `null` to the field's type. Keep
explicit nulls and unknown fields in the raw capture even when typed decoding
would reject or discard them. Common row timestamps are required SQLite integers
in Unix epoch **milliseconds**, defaulted/updated by Drizzle's application hooks;
message JSON times encode epoch-millisecond finite numbers. Row update times and
message lifecycle times have separate purposes.
[Timestamp columns](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/schema.sql.ts#L3-L9),
[optional and time codecs](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/schema.ts#L18-L40).

Path codecs normalize Windows backslashes to `/` for storage; absolute paths can
use POSIX, Windows drive, or UNC syntax. Reading on Windows converts recognized
Windows absolute paths back to platform separators. The directory codec permits
an empty legacy value; path strings must therefore remain native metadata rather
than universally valid paths on the capture host.
[Path codecs](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/path.ts#L6-L79).

Session IDs use the native session namespace; current message IDs validate the
prefix `msg_`. Generated message IDs may derive from an event ID. IDs convey
identity; `seq` conveys transcript order. Preserve the source/store identity with
all native keys in an importer, including copied/forked sessions and exports.
[Message ID](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L21-L35).

## `session_v2`

This is the current table name despite the unversioned current public `Session`
contract. Unless marked required below, SQL columns are nullable. `JSON` means a
TEXT column interpreted as JSON.

| Columns | Native storage and meaning |
| --- | --- |
| `id` | TEXT primary key. |
| `project_id` | Required TEXT; FK `project.id`, delete cascade. |
| `workspace_id` | TEXT; optional placement identity. |
| `parent_id` | TEXT; delegation/child-session parent. No session FK declared. |
| `fork_session_id`, `fork_boundary` | TEXT source session ID and JSON `{type:"before"\|"through",messageID}`. Separate from `parent_id`; no session FK declared. |
| `slug`, `directory`, `version` | Required TEXT. Directory uses the native path column codec; version is the recorded application version. |
| `path`, `title`, `share_url` | TEXT; `path` is project-relative subpath. |
| `summary_additions`, `summary_deletions`, `summary_files` | INTEGER. |
| `summary_diffs` | JSON array of legacy file-diff objects. |
| `metadata` | JSON record of arbitrary JSON-serializable annotations. |
| `cost` | Required REAL, default 0; USD aggregate. |
| `tokens_input`, `tokens_output`, `tokens_reasoning`, `tokens_cache_read`, `tokens_cache_write` | Required INTEGER, default 0; aggregate token counts. |
| `revert` | JSON current or legacy revert state. |
| `permission` | JSON permission ruleset. |
| `agent` | TEXT agent ID. |
| `model` | JSON `{id, providerID, variant?}`. |
| `time_created`, `time_updated` | Required INTEGER epoch ms. |
| `time_busy`, `time_idle`, `time_viewed`, `time_compacting`, `time_archived` | INTEGER epoch ms. |
| `idle_outcome` | TEXT `succeeded`, `failed`, or `interrupted`. |
| `time_suspended` | INTEGER execution-claim timestamp; historical column name. |
| `resume_attempts` | Required INTEGER, default 0. |

[All session columns](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/sql.ts#L28-L84),
[fork boundary schema](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-fork.ts#L6-L10).

Public `Session.Info` renames `project_id` → `projectID`, `parent_id` → `parentID`,
`permission` → `permissions`, places directory/workspace in `location`, path in
`subpath`, tokens in a nested object, and the two fork fields in `fork`. Many
nullable columns become absent optional fields. A stored model with no variant
reads with `variant:"default"`; an empty stored `path` reads as absent `subpath`.
`slug`, `version`, share/summary columns, compaction and claim bookkeeping are
not returned by this mapper. Keep raw session rows for native fidelity.
[Exact mapper](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/info.ts#L12-L60).

Session metadata keys are opaque to core; children and forks ordinarily inherit
the parent's current metadata. Current revert JSON has required `messageID` and
optional `partID` (legacy compatibility), `snapshot`, `files`. Legacy revert may
also carry `diff`, and file objects use `path` where the current form uses `file`.
[Metadata](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-metadata.ts#L3-L11),
[revert and legacy decode](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-revert.ts#L7-L85).

## `session_message` and `session_part`

| Table | Fields and constraints |
| --- | --- |
| `session_message` | `id` TEXT PK; required `session_id` FK to `session_v2` with delete cascade; required `type` TEXT, `seq` INTEGER, `time_created`/`time_updated` INTEGER, `data` JSON TEXT. Unique `(session_id, seq)`. |
| `session_part` | Required `message_id` FK to `session_message` and `session_id` FK to `session_v2`, both delete cascade; required `ordinal` INTEGER, `type` TEXT, `time_created`/`time_updated` INTEGER, `data` JSON TEXT; nullable `call_id` TEXT. PK `(message_id, ordinal)`. |

The message row's `data` omits logical `id` and `type`; reconstruct them from the
row columns. The part row's `data` is an entire encoded assistant content object,
including `type`. `call_id` is the tool block's `id` for tool parts, otherwise
NULL. Part ordinal is zero-based on insertion and is an ordering key; preserve
gaps and actual values. It is scoped to the message, with no independent `prt_`
part identity in this architecture. `seq` is session-scoped and can have gaps
because it originates in the session's aggregate sequence; it is never an array
offset. Forks reserve inherited sequence positions, and imports renumber them.
[Table definitions](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/sql.ts#L21-L26),
[message and part tables](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/sql.ts#L86-L128),
[insertion](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L798-L835).

The exact read composition is:

1. Order message rows by native `seq` (explicit ascending for chronology).
2. For assistant rows, gather `session_part` rows ordered by `ordinal`.
3. If parts exist, use `parts.data` followed by any array-valued inline
   `message.data.content`; otherwise retain inline content unchanged.
4. Hydrate media, upgrade legacy field spellings, attach row `id`/`type`, and
   decode the logical message.

An assistant can therefore be inline, split (`data.content:[]` plus parts), or
mixed (parts plus nonempty inline content). Concatenation preserves both without
deduplication. Native `withParts` treats non-array inline content as absent when
parts exist. SQL part `type` is used by the standalone part decoder; full message
composition uses the `type` already inside each part's `data`. Preserve both if
inconsistent, and report the inconsistency separately.
[Merge and decode](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/history.ts#L90-L160),
[message pagination](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/store.ts#L101-L134).

## Logical message union

Every current logical message has required `id`, `type`, and `time.created`, plus
optional `metadata` (arbitrary string keys/unknown values). The table summarizes
additional fields. A `?` marks an optional JSON property; all other listed fields
are required. Model refs use `{providerID,id,variant?}`.

| `type` | Fields beyond the common base |
| --- | --- |
| `user` | `text`, `files?`, `agents?`, `skills?`. User input is a text field and attachment arrays. |
| `synthetic` | `text`, `description?`. |
| `system` | `text` frozen for the model at emission, `description?` for transcript display. |
| `skill` | `skill` ID, `name`, `text`. |
| `agent-switched` | `agent`, `previous?` agent ID. |
| `model-switched` | `model`, `previous?` model ref. |
| `location-switched` | `location:{directory,workspaceID?}`, `projectID?`, `subpath?`, `previous?:{location,projectID?,subpath?}`. |
| `shell` | `shellID`, `command`, `status`, `exit?`, `output?`; `time.completed?`. Status: `running`, `exited`, `timeout`, `killed`. |
| `assistant` | `agent`, `model`, `content:[]`; `snapshot?`, `finish?`, `rawFinish?`, `native?`, `cost?`, `tokens?`, `error?`, `retry?`; `time.streamed?`, `time.completed?`. |
| `compaction` | Tagged by `status`; see below. |
| `idle` | `outcome` (`succeeded`, `failed`, `interrupted`), `error?`. |

[Base and non-assistant messages](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L31-L133),
[assistant and compaction](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L220-L289),
[idle and complete union](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L291-L329),
[model reference](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/model.ts#L19-L24).

Assistant `snapshot` has optional `start`, `end`, `files` (relative-path array).
`finish` is `stop`, `length`, `tool-calls`, `content-filter`, `error`, or `unknown`;
`rawFinish` retains the producer's raw string. `retry` is `{attempt,at,error}`
with positive-integer attempt and epoch-ms `at`. `time.streamed` marks the end
of the provider response body before tool settlement. An `idle` record groups
steps since the previous marker into a native assistant turn; shutdown does not
append an idle marker.
[Assistant fields and idle semantics](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L213-L302),
[finish literals](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/llm.ts#L5-L6).

`shell.output`, when present, is `{output:string,cursor:int,size:int,truncated:bool}`.
Cursor and size are byte positions/counts for captured output, so the text can
be a bounded page. Preserve the truncation flag and metadata rather than treating
that text as proof the whole shell output was captured.
[Shell output contract](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/shell.ts#L77-L85).

## Assistant blocks and tool call/result pairing

| Block `type` | Required and optional fields |
| --- | --- |
| `text` | `text`, `native?`. |
| `reasoning` | `text`, `native?`, `time?:{created,completed?}`. |
| `tool` | `id` (call ID string), `name`, `state`, `time:{created,ran?,completed?}`, `executed?` boolean, `native?`. |

`native` is an opaque provider-owned string-keyed record echoed for replay;
preserve signatures, encrypted reasoning and other fields unchanged. Tool result
native data merges into call native data. The same tool block stores the call
and eventual result; `id` binds them within its assistant message. Pair using
source + session + message + call identity, preserving ordinal and name. A tool
result is not a separate top-level `role:"tool"` record in this schema.
[Native definition](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L37-L44),
[all assistant block shapes](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L135-L211).

| `state.status` | State fields |
| --- | --- |
| `streaming` | `input` string containing streamed arguments. |
| `running` | `input` object (string keys/unknown values), required `metadata` JSON record. |
| `completed` | `input` object, required nonempty `content` array, optional `metadata`. |
| `error` | `input` object, required structured `error`, optional nonempty `content`, optional `metadata`. |

Tool result content is an ordered union of `{type:"text",text}` and
`{type:"file",uri,mime,name?}`. A file URI can be `data:`, `media:`, or another
string accepted by the tool content contract. Error shape is
`{type:string,message:string,status?:100..599,response?:{body:string},retryAfterMs?:nonnegative_int}`;
retry duration is **milliseconds**. Preserve state metadata and native replay
data independently of result text.
[Tool states](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L135-L183),
[tool content](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/tool.ts#L67-L84),
[structured error](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-error.ts#L6-L14).

`cost` is USD, not microdollars. `tokens` is
`{input,output,reasoning,cache:{read,write}}`, each finite numeric in the JSON
contract (session aggregate SQL columns are integers). Normalization uses
noncached input, visible output, reasoning, cache-read input and cache-write
input respectively; `total` sums those five buckets. Provider-reported cost,
including zero, takes precedence over catalog price. Session aggregate totals
are native observations, and forks initially reset them to zero while copied
message usage remains present. Keep message and session usage separately.
[Usage contract](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/token-usage.ts#L5-L18),
[normalization and cost](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/usage.ts#L8-L50),
[fork totals](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L162-L188).

## Compaction and instructions

All compaction states have `reason:"auto"|"manual"`. Running has `summary`;
completed has `summary`, optional `model`, `native`, `cost`, `tokens`; failed
has required `error`, optional `cost`, `tokens`, and no required summary.
Compaction usage describes the compaction request itself. Completed provider
compactions store canonical AI-message JSON windows in `native.messages`, replayed
instead of the summary. Summary compactions may refer to retained history with
`metadata.keepID`; model-context reads rebuild `metadata.recent` in memory from
those rows. Full message enumeration retains the original rows before the
compaction. Native windows are opaque structured content whose own media can be
nested; the blob collector's recognized locations are narrower (see lifecycle).
[Compaction contract](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-message.ts#L248-L289),
[native windows](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/history.ts#L27-L53),
[kept text](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/history.ts#L236-L274).

Instruction tables supplement transcript `system` notices:

| Table | Native columns |
| --- | --- |
| `instruction_entry` | PK `(session_id,key)`; required session FK (delete cascade), TEXT key; nullable JSON `value`; required boolean-mode INTEGER `removed` default false; required timestamps. |
| `instruction_blob` | TEXT `hash` PK; nullable JSON `value`. |
| `instruction_state` | `session_id` PK/FK (delete cascade); required INTEGER `epoch_start`, `through_seq`; required JSON `initial_values`, `current_values`. |

Values maps are instruction key → 64-lowercase-hex hash; deltas use a hash or
literal `"removed"`. A key matches the native slash-separated namespace pattern.
Instruction updates can render frozen text into `system` messages; an initial
baseline can exist without a visible system message. Capture referenced instruction
blob JSON alongside these maps when preserving the stored instruction context.
[Instruction tables](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/sql.ts#L154-L183),
[key/hash/value contracts](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/instruction.ts#L5-L25),
[baseline and frozen text](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/instruction-state.ts#L38-L60).

## Synthetic central examples

These are complete logical user and assistant message shapes according to the
reviewed declarations; optional fields are deliberately omitted. IDs, model and
tool names are synthetic strings. Effect schema validation was not executed.

```json
{
  "id": "msg_example_user",
  "type": "user",
  "time": {"created": 1794000000000},
  "text": "Show the greeting."
}
```

```json
{
  "id": "msg_example_assistant",
  "type": "assistant",
  "agent": "build",
  "model": {"providerID": "example", "id": "example-model"},
  "content": [
    {"type": "reasoning", "text": "Read the sample."},
    {
      "type": "tool",
      "id": "call_example",
      "name": "read",
      "state": {
        "status": "completed",
        "input": {"filePath": "hello.txt"},
        "content": [{"type": "text", "text": "Hello"}]
      },
      "time": {"created": 1794000000100, "ran": 1794000000110, "completed": 1794000000120}
    },
    {"type": "text", "text": "The greeting is Hello."}
  ],
  "finish": "stop",
  "cost": 0,
  "tokens": {"input": 12, "output": 8, "reasoning": 3, "cache": {"read": 0, "write": 0}},
  "time": {"created": 1794000000100, "streamed": 1794000000130, "completed": 1794000000140}
}
```

This next JSON is a **documentation envelope**, not a native export. Each nested
row is a full reduced-content SQL row with JSON TEXT shown decoded. It demonstrates
mixed storage: the reconstructed content is `"First."` followed by `"Second."`.
The matching session/project rows are omitted.

```json
{
  "session_message": {
    "id": "msg_example_mixed",
    "session_id": "ses_example",
    "type": "assistant",
    "seq": 7,
    "time_created": 1794000000000,
    "time_updated": 1794000000100,
    "data": {
      "agent": "build",
      "model": {"providerID": "example", "id": "example-model"},
      "content": [{"type": "text", "text": "Second."}],
      "time": {"created": 1794000000000, "completed": 1794000000100}
    }
  },
  "session_part": [
    {
      "message_id": "msg_example_mixed",
      "session_id": "ses_example",
      "ordinal": 0,
      "type": "text",
      "call_id": null,
      "time_created": 1794000000000,
      "time_updated": 1794000000050,
      "data": {"type": "text", "text": "First."}
    }
  ]
}
```

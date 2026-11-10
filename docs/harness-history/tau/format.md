# Tau stored fields and codec

This is the native Python Tau schema at the revision in [README](README.md). Names below are literal canonical persisted names. Do not mechanically apply Pi's session-v3 envelope or camelCase entry fields to Tau.

## Serialization, validation and defaults

The durable storage codec uses a Pydantic `TypeAdapter[SessionEntry]` and `dump_json(entry, exclude_none=True)`, followed by a newline. Outer entry classes inherit `extra="forbid"` and have no alias generator: `parent_id`, `custom_type`, `first_kept_entry_id` and other outer names remain snake_case. Nested `WireModel` classes also forbid unknown fields, accept both Python snake_case names and camelCase aliases at validation, and serialize by their camelCase aliases. Canonical output omits `None` model fields at each nested model level (JSON null values inside arbitrary data dictionaries remain values) and emits non-None defaults (including false, empty arrays and zero-valued usage). The special empty `replaces_entry_ids` compatibility field is omitted even when explicitly set. [Codec](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/jsonl.py#L12-L32), [outer schema](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/entries.py#L25-L49), [WireModel config](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/messages.py#L14-L56).

Defaults and coercions matter for archival use. Absent `id`/`timestamp` are generated on model construction/validation; absent `parent_id` becomes null in memory and is omitted by canonical serialization. Optional fields absent or JSON null generally become Python None. Required nonnullable fields reject null; Pydantic validation is not globally strict-type mode, so do not describe accepted input as byte- or type-exact. Unknown type/role/block discriminators or extra schema fields raise a wrapped `SessionJsonlError`; arbitrary JSON is allowed only in declared open payload regions. A future schema may therefore fail this release's strict reader. Preserve raw records before model validation. [Entry defaults](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/entries.py#L15-L39), [decoder errors](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/jsonl.py#L24-L60).

**Presence is semantic for compaction:** state replay checks whether `replaces_entry_ids` was explicitly supplied (`model_fields_set`), even when its value is `[]`. Explicit `[]` retains previous messages and appends the summary; absent list uses the inclusive `first_kept_entry_id` boundary. Canonical serialization omits an empty list, so normalizing/re-exporting a legacy explicit-empty record can lose that distinction. Store original field presence alongside raw bytes. [Replay precedence](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/memory.py#L151-L197), [exclude-if rule](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/entries.py#L72-L85), [upstream tests](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/tests/test_session.py#L561-L581), [explicit-empty replay test](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/tests/test_session.py#L810-L837).

The user-facing JSONL exporter is a related serialization surface: it calls `entry.model_dump_json()` without `exclude_none=True`. Export can include explicit nulls absent from storage output, while still using nested aliases and the empty-list exclusion rule. Preserve which producer created a file. [Export serializer](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_coding/session_export.py#L74-L84).

## Common entry envelope and identity

Every entry has:

| Literal field | Model/default and stored representation |
| --- | --- |
| `type` | Required discriminated variant (each constructor has its literal default) |
| `id` | String; default UUIDv4 `.hex` (32 hexadecimal characters), native entry occurrence ID |
| `parent_id` | String or None; default None, omitted by canonical storage when None |
| `timestamp` | Float; `time.time()` Unix epoch **seconds**, potentially fractional |

There is no `type: session` header, no session-level ID in every line, and no numeric file version gate in this codec. `session_info` is a normal tree entry. The coding-session ID comes from an index/config record or filename convention, distinct from entry IDs and tool/provider IDs. Ordinary message entries receive a new append-time seconds timestamp independently of nested message time; custom runtime message persistence explicitly derives outer seconds from nested milliseconds. [Entry base](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/entries.py#L15-L39), [message-to-entry writer](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_coding/session.py#L4251-L4264), [index](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_coding/session_manager.py#L56-L120).

## Outer variants

The table lists fields beyond the common envelope. `=...` gives model default, not a guarantee of on-disk omission; None disappears only in canonical storage. Optional model fields may have explicit JSON null in exports.

| `type` | Additional literal fields |
| --- | --- |
| `message` | Required `message: AgentMessage` |
| `custom_message` | Required `custom_type: string`, `content: UserContent`; `display: bool=true`; `details: JSONValue=None` |
| `model_change` | Required `model: string`; `provider: string or None=None` (older records may omit provider) |
| `thinking_level_change` | `thinking_level: string or None=None` |
| `compaction` | Required `summary: string`; `replaces_entry_ids: string[]=[]` (legacy, empty excluded); `first_kept_entry_id: string or None=None`; `tokens_before: int or None=None`; `usage: Usage or None=None`; `provider`, `model`, `response_provider`: string or None, each default None |
| `branch_summary` | Required `summary: string`; `branch_root_id: string or None=None`; same optional `usage`, `provider`, `model`, `response_provider` as compaction |
| `label` | Required `target_id: string`; `label: string or None=None` |
| `leaf` | Legacy accepted shape: `entry_id: string or None=None`; current replay ignores this as active-tip selection |
| `session_info` | `created_at: float=time.time()`; `cwd`, `title`: string or None, default None |
| `custom` | Required `namespace: string`; `data: object<string, JSONValue>={}` |

`session_info.created_at` and its outer `timestamp` are seconds and are independently defaulted. There is no dedicated `usage` or `context_edit` outer variant in this union. Summary usage is retained in the summary record; message usage in its nested assistant. Custom entries retain application state on the active path and contribute no message. Custom-message entries contribute a runtime custom role regardless of `display`. Branch-summary `branch_root_id` identifies the branch destination/root selected by the writer, while its `parent_id` links it into the tree. [Complete entry models](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/entries.py#L35-L144), [custom replay](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/memory.py#L69-L110), [summary writer](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_coding/session.py#L1053-L1089).

## Nested message protocol

Python attribute names are snake_case; **stored nested names below use aliases**. In particular outer `response_provider` and nested `responseProvider` are distinct locations with distinct naming. Every built-in nested message has integer Unix-millisecond `timestamp` defaulted with `int(time()*1000)`.

| Nested `message.role` | Literal fields beyond role |
| --- | --- |
| `user` | Required `content: string or text/image block array`; `timestamp` |
| `assistant` | `content: ordered text/thinking/toolCall array=[]`; `api="unknown"`, `provider="unknown"`, `model="unknown"`; optional `responseModel`, `responseProvider`, `responseId`, `diagnostics`, `timing`, `errorMessage`; `usage=zero Usage`, `stopReason="stop"`, `timestamp` |
| `toolResult` | Required `toolCallId`, `toolName`; `content: text/image array=[]`; `details: JSONValue=None`; `addedToolNames: string[] or None=None`; `isError=false`; `timestamp` |
| `bashExecution` | Required `command`, `output`; `exitCode: int or None=None`, `cancelled=false`, `truncated=false`, `fullOutputPath: string or None=None`, `excludeFromContext=false`, `timestamp` |
| `custom` | Required `customType`, `content: UserContent`; `display=true`, `details: JSONValue=None`, `timestamp` |
| `branchSummary` | Required `summary`, `fromId: string`; `timestamp` |
| `compactionSummary` | Required `summary`, `tokensBefore: int`; `timestamp` |

`AgentMessage` is a discriminated union of these seven roles. The canonical coding writer converts runtime custom messages to top-level `custom_message` entries, with seconds timestamp derived from the runtime timestamp; accepted old nested custom forms migrate at decode. The union still accepts nested summary roles, whereas normal compaction and branch-summary operations persist top-level summary entries. This release has no nested `system` role in AgentMessage; its system prompt is assembled separately by CodingSession. [Full message schema](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/messages.py#L130-L284), [canonical custom writer](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_coding/session.py#L4251-L4264), [migration](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/jsonl.py#L85-L100).

`stopReason` accepts `stop`, `length`, `toolUse`, `error`, `aborted`. Python construction/validation accepts assistant string content as a convenience and converts it to a text block array (empty string → empty array); missing/null usage normalizes to zero Usage. Similar tool-result string content converts to text blocks. Persisted canonical output is block based. [Normalizers](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/messages.py#L154-L229).

| Block `type` | Literal stored keys and defaults |
| --- | --- |
| `text` | Required `text`; optional `textSignature` |
| `thinking` | Required `thinking`; optional `thinkingSignature`; `redacted=false` |
| `image` | Required `data`, `mimeType` |
| `toolCall` | Required `id`, `name`; `arguments={}` JSON object; optional `thoughtSignature` |

User/custom and tool-result block arrays allow text/images; assistant block arrays allow text/thinking/calls. Images embed base64-compatible data strings and MIME types; the schema itself types the image body as a string. Tool calls remain inside assistant content and results are linked separately by `toolCallId`. Content arrays preserve ordering. Signatures carry provider replay data; keep exact strings. Core protocol has no audio/video block. [Blocks](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/messages.py#L96-L127), [test image round-trip](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/tests/test_session.py#L121-L162).

## Usage, diagnostics and timing

Nested `Usage` serializes `input`, `output`, `cacheRead`, `cacheWrite`, `totalTokens` (integers default 0), optional integer `cacheWrite1h`, `reasoning`, plus `cost`. Cost serializes `input`, `output`, `cacheRead`, `cacheWrite`, `total` (floats default 0.0), documented as USD. Summary-level Usage uses the same nested camelCase aliases. Do not add optional reasoning or cacheWrite1h to totals a second time without the matching accounting semantics. Assistant `responseProvider` can differ from requested provider/routing metadata. [Usage models](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/messages.py#L36-L86), [summary alias test](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/tests/test_session.py#L622-L640).

`diagnostics[]` stores `type`, integer millisecond `timestamp`, optional `error` (`name?`, required `message`, `stack?`, `code?: string or int`) and `details?: object<string, JSONValue>`. `timing` stores required nonnegative `totalDurationMs`, optional nonnegative `timeToFirstOutputMs`; these are monotonic elapsed durations in milliseconds. The optional fields are omitted by canonical storage when None. [Diagnostic/timing types](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/messages.py#L89-L94), [diagnostics](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/messages.py#L140-L173), [timing round-trip](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/tests/test_session.py#L103-L120).

| Location | Time meaning/unit |
| --- | --- |
| Outer entry `timestamp`, session_info `created_at` | Unix seconds, float |
| Index `created_at`, `updated_at` | Unix seconds, float |
| Any nested message `timestamp` | Unix milliseconds, integer |
| Nested diagnostics `timestamp` | Unix milliseconds, integer |
| `timing.totalDurationMs`, `timeToFirstOutputMs` | Elapsed milliseconds |
| Runtime custom reconstructed from top-level custom_message | `round(outer timestamp * 1000)` |
| Runtime summary UserMessage synthesized by SessionState | New default millisecond time at replay; not the stored summary's outer timestamp |
| Custom/details/arguments fields | Application-defined; no implicit time conversion |

That final synthesized-message distinction matters for byte- or timestamp-stable replays: current compaction/branch-summary state construction creates UserMessage without supplying timestamp. Preserve the original summary record's seconds timestamp as native evidence and avoid treating synthesized runtime time as occurrence time. [Replay creation](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/memory.py#L73-L110), [compaction UserMessage](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/session/memory.py#L151-L157), [timestamp defaults](https://github.com/huggingface/tau/blob/47610d3ed71deb70b07443a8641588ffc9c8ffc6/src/tau_agent/messages.py#L19-L21).

# OpenCode compatibility and older native shapes

Last reviewed: **2026-11-07** at `dev` commit
`c33eaa170ed5093c08f1684862a695aae036dcda` (root version `2.2.5`).
This describes compatibility code present at that pin. Historical coverage is
bounded by the retained source; a future adapter should detect actual schema
features and preserve unknown data before applying an interpretation.

## Database schema identity

The current runtime has a `migration(id TEXT PRIMARY KEY,time_completed INTEGER
NOT NULL)` journal. Existing databases can be bootstrapped from
`__drizzle_migrations` by names or timestamps. Empty databases bootstrap the
generated schema and mark migrations complete. Presence of `session` or
`session_v2` selects the migration path. The session's `version` column is an
application version, while migration journal + actual table/column definitions
describe database layout. Neither the filename nor package version alone is a
complete schema discriminator.
[Migration journal and bootstrap](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/migration.ts#L24-L110).

The `session_part` migration creates an empty table/index with no backfill. Legacy
inline rows remain possible after its presence. The current writer also supports
lazy conversion and mixed rows, so discovering the table does not establish that
all assistant content lives there.
[Part-table migration](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/migration/20261029002625_session_part.ts#L4-L26),
[mixed reader](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/history.ts#L90-L123).

## Current-architecture JSON written before v2.1

These are compatibility aliases within the current architecture, separate from
the V1 message/part schema below. Stored projected messages and exports keep the
spelling written at the time. `SessionLegacy.message/part` upgrades them in memory:

| Stored older field | Current interpretation |
| --- | --- |
| Assistant message `providerState` | `native`. |
| Text/reasoning block `state` | `native`. |
| Tool block `providerState` | `native`; tool `state` remains lifecycle data. |
| Tool `providerResultState` | Removed and merged over the tool's `native` record; result keys win. |
| Compaction `providerState` | `native`, subject to provider-context replacement below. |
| Compaction `recent` | Removed; nonempty string becomes `metadata.recent`, existing metadata keys win. |
| Compaction `providerContext` object | Removed; becomes `native:{messages:...}`, replacing sibling native data. |

On ordinary renames, an existing current property wins even when it contains
null or another unusual value: the adapter checks property presence. A
non-record `providerResultState` is simply removed. `providerContext.messages`
is mapped when an array, otherwise forwarded as its observed value. Before
`2.0.15`, native-window media parts could store `mediaType` and base64 `data`
directly; the adapter moves them to
`media:{source:{type:"base64",data,mediaType}}` when no `media` is already present.
[Complete compatibility adapter](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-legacy.ts#L3-L116).

Preserve original JSON as observed and keep normalization separately versioned.
For example, the following authored complete text-block shape is accepted through
the compatibility path and normalizes `state` to `native`:

```json
{"type": "text", "text": "Hello", "state": {"exampleReplayKey": "opaque"}}
```

## V1 SQL transcript layout

The retained SQL migration defines:

| Table | Original columns relevant to the transcript |
| --- | --- |
| `session` | `id` TEXT PK; required `project_id`, `slug`, `directory`, `title`, `version`; nullable `parent_id`, share/summary fields, JSON revert/permission; required created/updated timestamps; nullable compacting/archived timestamps. Later migrations add fields. |
| `message` | `id` TEXT PK; required `session_id` FK to `session` (delete cascade), integer created/updated timestamps, JSON TEXT `data`. |
| `part` | `id` TEXT PK; required `message_id` FK to `message` (delete cascade), `session_id`, integer created/updated timestamps, JSON TEXT `data`. |

V1 `part` has a native `prt...` identity and no ordinal column; current
`session_part` uses `(message_id,ordinal)`. V1 message JSON is tagged by `role`
(`user` or `assistant`), current message JSON by row `type`. The retained migration
reconstructs identities from SQL columns before decoding, sorts source messages
by `(time_created,id)` and parts lexically by ID, then assigns projected sequences.
Those are the reviewed migration's ordering rules; retain the native V1 keys.
[Original SQL](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/migration/20260127222353_familiar_lady_ursula.ts#L22-L73),
[V1 identity/ordering](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/v1-migration.bun.ts#L227-L275).

V1 fields below use `?` for optional properties and epoch-ms numeric time fields.
The retained validators permit message prefixes `msg` and part prefixes `prt`
(looser than requiring the underscore). SQL identities are authoritative in the
migration reconstruction.
[V1 IDs and part base](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/v1/session.ts#L15-L27),
[part identity](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/v1/session.ts#L81-L85).

| V1 message | Native logical fields |
| --- | --- |
| User | `id,sessionID,role:"user",time:{created},agent,model:{providerID,modelID,variant?}`; optional `format`, `summary:{title?,body?,diffs:[]}`, `system`, `tools` boolean map. `format` is `text` or `json_schema` with schema and optional/defaulted retry count. |
| Assistant | `id,sessionID,role:"assistant",time:{created,completed?},parentID,modelID,providerID,mode,agent,path:{cwd,root},cost,tokens`; optional `error,summary,structured,variant,finish`. Tokens have `input,output,reasoning,cache:{read,write}`, optional `total`. |

V1 text and attachments are owned part records rather than user-message text.
Assistant `parentID` points to a message, while session `parent_id` points to a
session. Preserve both relationships with their native types.
[V1 user](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/v1/session.ts#L327-L355),
[V1 assistant](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/v1/session.ts#L453-L491).

Every V1 logical part carries `id,sessionID,messageID,type` plus:

| `type` | Native fields |
| --- | --- |
| `text` | `text`, `synthetic?`, `ignored?`, `time?:{start,end?}`, `metadata?`. |
| `reasoning` | `text`, `metadata?`, `time:{start,end?}`. |
| `file` | `mime`, `url`, `filename?`, `source?`. |
| `agent` | `name`, optional source mention `{value,start,end}`. |
| `snapshot` | `snapshot` string. |
| `patch` | `hash`, `files` array. |
| `step-start` | `snapshot?`. |
| `step-finish` | `reason`, `snapshot?`, `cost`, `tokens` (same buckets plus optional total). |
| `compaction` | `auto` boolean, `overflow?` boolean, `tail_start_id?` message ID. |
| `subtask` | `prompt`, `description`, `agent`, `model?:{providerID,modelID}`, `command?`. |
| `retry` | `attempt` nonnegative integer, structured V1 API `error`, `time:{created}`. |
| `tool` | `callID`, `tool`, `state`, `metadata?`. |

File `source` can be `file` (path), `symbol` (path/range/name/kind), or `resource`
(clientName/uri), all carrying source text `{value,start,end}`. Preserve the URL
and source metadata; data URIs may carry the actual media bytes.
[V1 blocks and attachments](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/v1/session.ts#L87-L257),
[V1 tool](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/v1/session.ts#L259-L325).

| V1 tool `state.status` | State fields |
| --- | --- |
| `pending` | `input` object, `raw` string. |
| `running` | `input`, `title?`, `metadata?`, `time:{start}`. |
| `completed` | `input`, `output` string, `title`, `metadata`, `time:{start,end,compacted?}`, `attachments?:FilePart[]`. |
| `error` | `input`, `error` string, `metadata?`, `time:{start,end}`. |

V1 tool error text and output strings differ from current structured errors and
content arrays. Preserve `compacted` and any attachment references, because a
pruned native result may have less retained content than its earlier revision.
[V1 state declarations](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/v1/session.ts#L259-L313).

## Migration behavior and capture boundaries

The migration reads V1 rows and transforms them into current inline messages.
Invalid/orphan parts and invalid messages generate warnings and can be skipped.
User synthetic/ignored parts, subtask wrappers, compaction pairs, media, snapshots
and tool names receive semantic transformations. V1 `bash`, `task`, `apply_patch`
are recognized as renamed tools (`shell`, `subagent`, `patch`), and some V1
fields have no direct current equivalent. For complete native capture, retain
V1 raw rows as well as any already migrated current rows and record which
representation each came from. Normalized similarity can help relate versions;
it cannot serve as their sole identity.
[V1 message/media transform](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/v1-migration.bun.ts#L278-L445),
[renamed tool constants](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/v1-migration.bun.ts#L996-L1004),
[per-session copy](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/v1-migration.bun.ts#L673-L721).

The retained previous-next importer defaults to `opencode-next.db`, opens it
read-only and checks for `project`, `session`, `session_message`. It imports the
older current-architecture session/message shapes, copying IDs, sequences,
timestamps and JSON. Some session columns are required; nullable and fallback
columns are projected explicitly for older schemas. This is a separate historical
source from the current shared database and may overlap migrated sessions.
[Next source discovery](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/v1-migration.bun.ts#L799-L836),
[copy](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/v1-migration.bun.ts#L858-L924),
[feature detection](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/v1-migration.bun.ts#L936-L972).

Earlier filesystem JSON storage is not specified here: this reviewed checkout's
retained compatibility paths establish SQL/V1/current shapes, but do not supply
a full authoritative version matrix for those earlier file layouts. Such sources
need their own pinned historical review before claiming complete compatibility.

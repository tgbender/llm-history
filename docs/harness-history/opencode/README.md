# OpenCode durable conversation history

Last reviewed: **2026-11-07**. Reviewed upstream branch: **`dev`**; exact commit:
**`c33eaa170ed5093c08f1684862a695aae036dcda`**; root package version:
**`2.2.5`**. The version identifies the reviewed source tree; development builds and
installed databases can differ. This is a point-in-time source specification.
[Version evidence](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/package.json#L1-L8).

OpenCode's current durable transcript is a mutable SQLite projection: session rows,
ordered message rows, assistant content stored inline or in ordered part rows, and
optional shared media blobs. A complete transcript capture reads all messages,
including the history before compaction, and captures media dependencies in the
same consistent snapshot. These documents describe native facts and separately
identify implications for this project's parser/storage design.

Read [the persisted schema](schema.md), [lifecycle and acquisition](lifecycle.md),
and [compatibility](compatibility.md). All examples are authored synthetic data.
JSON examples identify whether they are complete logical shapes or reduced views;
none are copied from personal histories.

## Locations and discovery

The native Node/Bun global data root is
`$XDG_DATA_HOME/opencode`, defaulting to `~/.local/share/opencode`. This applies to
the reviewed platform-neutral XDG implementation, including macOS. Cache, config,
and state have separate XDG roots. `OPENCODE_CONFIG_DIR` changes the config root;
`OPENCODE_TEST_HOME` changes the service's `home` property and does not recompute
the module-initialized data root.
[Root resolution](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/util/src/global-roots.ts#L4-L18),
[global paths and overrides](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/util/src/global.ts#L12-L26),
[config override](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/util/src/global.ts#L77-L84).

The CLI selects `opencode.db` for channels `latest`, `dev`, `beta`, `next`, and
`prod`; other channels use `opencode-<sanitized-channel>.db` (characters outside
`[a-zA-Z0-9._-]` become `-`). `OPENCODE_DISABLE_CHANNEL_DB=1` or `true` selects
the shared name. `OPENCODE_DB` overrides that selection and can be an absolute
path, data-root-relative path, or `:memory:`. Embedded runtimes can inject their
own database path/client; a remote server's history belongs to its storage host.
[CLI database path](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/cli/src/database-path.ts#L4-L13),
[database configuration and injected clients](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/database.ts#L69-L81),
[client layer](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/database.ts#L102-L121).

Discovery should therefore inventory configured sources and relevant channel files,
then inspect `sqlite_master`, `PRAGMA table_info`, and migration records read-only.
The filename alone does not identify the format. Historical `opencode-next.db`
is recognized by the migration importer. Current and V1 session tables can
coexist during migration; see [compatibility](compatibility.md).
[Previous-next import](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/v1-migration.bun.ts#L799-L836).

Sessions span projects/workspaces and child sessions. Enumerate every `session_v2`
row rather than only the current directory or top-level sessions. Archived
sessions stay in the same table: `time_archived IS NOT NULL` identifies them.
The native store includes both archive states when `archived` is omitted; its
session order is `(time_updated, id)`, default descending, with optional limits
and filters. A UI or API page can therefore cover only part of the source.
[Session list implementation](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/store.ts#L145-L186).

## Acquisition surfaces and their fidelity

| Surface | What it contains / transforms |
| --- | --- |
| SQLite snapshot | Native rows, JSON spellings, inline/split layout, pending prompts and binary blobs. Captures unsettled messages too. |
| `SessionStore.messages` / messages API | Message schema objects, ordered by `seq`; merges parts, upgrades legacy field names and hydrates media. Default store order is descending. |
| `SessionStore.context` | A model context bounded by the latest applicable completed compaction, with rebuilt kept text and instruction-related filtering. |
| Native session export | JSON object `{info, messages}`; all messages requested ascending, then filtered to settled messages. Media is hydrated through ordinary message reads. Optional sanitization replaces content. |

The native export excludes assistant messages without `time.completed` and shell
or compaction messages whose `status` is `running`. Import preserves message IDs
but assigns new `seq` values `index + 1`, resolves the destination project/location,
and stores imported messages inline. Export has no separate format-version field
in `SessionTransfer.Data`. It also omits the raw session columns, instruction
tables, part-row identities, blob references, and pending inbox. Preserve its
provenance as an export when ingesting it.
[Transfer shape](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-transfer.ts#L7-L11),
[export and import row construction](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/transfer.ts#L60-L98),
[settlement filter](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/transfer.ts#L204-L207),
[CLI JSON serialization](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/cli/src/commands/handlers/session/export.ts#L69-L74).

## Evidence and confidence

Confidence is high for the pinned current table definitions, reader merge order,
mutation paths, export filter, compatibility adapters, and media collection rules:
these were checked directly against implementation and relevant existing tests.
Evidence links use immutable upstream commit permalinks with line anchors.
Upstream tests were **read, not executed**; no dependencies were installed and no
personal database or running OpenCode service was accessed. JSON examples were
syntax-checked locally; native Effect validators were not run.

Coverage is strongest for the current SQLite architecture and the V1 shapes
retained in this checkout. Earlier standalone JSON-file storage, provider-specific
opaque `native` payloads, all plugin metadata conventions, and every historical
release's behavior are outside the reviewed evidence. Unknown columns, JSON
fields, and record kinds should remain capturable for later interpretation.

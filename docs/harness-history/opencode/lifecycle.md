# OpenCode lifecycle, media and capture implications

Last reviewed: **2026-11-07** at `dev` commit
`c33eaa170ed5093c08f1684862a695aae036dcda` (root version `2.2.5`).
Native facts below are distinguished from proposed importer behavior.

## Admission and mutable messages

`session_inbox` stores admitted, unconsumed prompts/controls. Its fields are
`id` TEXT PK, required `session_id` FK to `session_v2` (delete cascade), `type`
TEXT, `payload` JSON TEXT, `delivery` TEXT, `enqueued_seq` INTEGER, and
`time_created` INTEGER epoch ms. `(session_id,enqueued_seq)` is unique. Delivery
consumes the inbox row and creates a visible message in one projection transaction.
Pending user input can therefore be durable before it is visible in the transcript.
Keep pending inputs labeled as pending when capturing them; they are distinct
from delivered user-message occurrences.
[Inbox table](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/sql.ts#L130-L152),
[delivery projection](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L1008-L1069).

Inbox `delivery` is `steer` or `queue`. Types are `user` (prompt text and optional
files/agents/skills/metadata), `synthetic` (text, optional description/metadata),
`compaction` (empty payload), and `move` (`location`, `projectID`, optional
`subpath`). Stored user files can use the same blob-reference form as delivered
messages. Keep the admitted `id`, enqueue sequence and delivery mode with these
pending payloads.
[Inbox payload union](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/session-inbox.ts#L11-L57),
[stored user payload](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/stored.ts#L36-L44).

Delivered user/synthetic messages reuse the inbox ID but receive `time.created`
from the delivery event. Preserve an observed inbox enqueue time separately.
Assistant creation time comes from the step's `started` value; restarting a step
under an existing assistant message ID resets that message's created/streamed/
completed/error/retry/finish fields in place. Native identity therefore remains
useful even when a message's lifecycle timestamps change.
[Delivered time and identity](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L1010-L1034),
[assistant reuse and creation](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/message-updater.ts#L220-L261).

Assistant messages, tool blocks, shells, and compactions update in place. Tool
arguments progress from a streaming string to an object; result content and
completion times arrive later in the same block. Text/reasoning finalization,
retry state and terminal usage mutate an existing message. Re-reading only IDs
or sequences greater than the last captured message misses those changes.
[Updater state transitions](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/message-updater.ts#L220-L430),
[retry/compaction updates](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/message-updater.ts#L412-L481).

The reviewed insert path writes nonempty assistant content to `session_part` and
keeps `content:[]` in the message envelope. The update path is selected by
`experimental.part_rows === true` in the in-scope config; no config selects the
whole-message path. That path rewrites inline messages and, for messages already
split, updates existing part rows and retains the empty envelope. The split update
path lazily moves inline content into rows when writing parts. On mixed messages,
it appends carried inline blocks after the existing highest ordinal. Read
composition always supports both layouts independently of the current flag.
[Update-path selection](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L316-L339),
[whole-message writes](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L486-L560),
[lazy conversion](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L605-L670),
[insert path](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L798-L835).

An ordinal identifies the current position within a message; replacement can
delete and recreate rows. The whole-message path updates changed part JSON,
appends at highest existing ordinal + 1, and deletes stale trailing rows. The
split replacement path deletes all parts and recreates zero-based ordinals. A
capture revision therefore needs the whole observed ordered list and original
row keys, rather than assuming a part's body is permanently fixed by its ordinal.
[Changed/stale rows](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L500-L537),
[replacement](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L768-L787).

## Forks and delegation

Fork boundary JSON is `{type:"before"|"through",messageID}`. Fork projection
resolves the boundary's sequence and physically copies the qualifying history.
The new session has `parent_id:NULL`, `fork_session_id` set to the source, its own
slug and creation/update times, copied project/location/agent/model/metadata/
permissions/version, and zeroed aggregate cost/tokens. Delegated children instead
use `parent_id`. These are distinct relationships.
[Fork header and selection](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L124-L198).

Only settled assistant, shell and compaction rows are copied. Copied rows retain
their original sequence and row times; IDs become
`<msg-ID-derived-from-fork-event>_<original-seq>`. Split parts retain ordinals,
tool call IDs and row times under the remapped message ID. Summary
`metadata.keepID` is remapped by the referenced parent message's sequence. The
child reserves the copied sequence range before future messages. Parent and
child store independent copies of the transcript; ancestry is still valuable
metadata even if the source session subsequently disappears.
[Copy/remap/reservation](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L198-L313).

The subagent tool creates child sessions with `parentID` equal to the caller's
session and records the child's session ID in progress/result metadata. Child
conversations start fresh according to the tool description. The `job` table can
also tie `session_id` to `child_session_id` with `notification_id` (a message ID),
job `id`, `kind`, title/status/output/error, recovery JSON and timestamps. Preserve
these native relationship hints where captured, without inferring that identical
call/result text represents the same delegated session.
[Subagent relationship](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/tool/plugin/subagent.ts#L154-L201),
[result metadata](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/tool/plugin/subagent.ts#L253-L264),
[job columns](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/job/sql.ts#L7-L27).

## Archive, revert and deletion

Archive/unarchive sets/clears `time_archived` and updates the session's
`time_updated`; messages remain in their tables. Revert first stages JSON on the
session. Clearing that state restores the ordinary view without deleting rows.
Committed revert resolves a target message, deletes transcript messages with
`seq >= target.seq`, deletes inbox entries at/after that sequence, clears staged
revert state, rewinds instruction state as needed, and releases media references
in that range. Message deletion cascades to part rows.
[Archive mutation](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L999-L1004),
[revert projection](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L1125-L1161),
[part FK](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/sql.ts#L107-L128).

Native `Session.remove` recursively removes sessions whose `parent_id` points to
the removed session, then publishes deletion. Forks have a separate relationship
and `parent_id:NULL`. Projection releases session blob references and deletes the
session row; transcript/inbox/instruction rows with cascading FKs disappear.
An importer can preserve previously captured revisions according to its own
retention policy; that preservation is a product decision distinct from native
source availability.
[Recursive removal](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session.ts#L391-L401),
[session deletion projection](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/projector.ts#L963-L967).

## Attachments and media blobs

User attachment fields are `mime`, `source:{type:"inline"}` or
`source:{type:"uri",uri}`, and optional `name`, `description`,
`mention:{start,end,text}`. Inline files have required base64 `data`; a stored
file reference removes `data` and adds `uri:"media:<hash>"`. Agents have
`{name,mention?}`; skills have `{id,name,text?,mention?}`. Preserve source URI and
mention offsets as native metadata; the URI documents attachment origin and can
coexist with captured bytes.
[Prompt attachments](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/schema/src/prompt.ts#L6-L99),
[stored reference union](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/stored.ts#L10-L44).

`media:<hash>` uses lowercase hex SHA-256 of the **raw binary bytes**, 64 digits;
it names database content, with no server route embedded. `blob` columns are
`hash` TEXT PK, required `mime` TEXT, `bytes` INTEGER, binary `data`,
`time_created` INTEGER epoch ms; nullable `width`, `height` INTEGER. The ordinary
writer computes `bytes` from the decoded buffer and inserts on conflict do
nothing. Consequently, equal bytes share the hash even when attachment names or
declared MIME differ. Preserve per-occurrence MIME/name/source separately and
also preserve the blob row's MIME and dimensions.
[Blob schema](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/sql.ts#L185-L195),
[hash computation](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L248-L249),
[take and insertion policy](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L293-L326).

With `experimental.media_blob` enabled, native writes externalize user files and
completed/error tool content files whose URI matches the base64 data-URI pattern.
Existing media refs pass through; other tool URIs remain unchanged. The collector
recognizes user `files`, assistant tool-state `content`, and standalone tool parts;
it does not recursively search arbitrary metadata or `native.messages`. Inline,
ref-backed and mixed media representations can coexist after flag changes,
imports, forks, or offline backfills. Reading must support them independently
of the current flag. Fork media policy is selected from the parent's location;
the copy's refs keep shared blobs alive.
[Collector locations](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L27-L47),
[flag and fork policy](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L262-L291),
[externalization and offline split](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L329-L408).

`blob_ref` has required `session_id`, `message_id`, `hash`, `seq`; PK
`(session_id,message_id,hash)`. Only session and hash have FKs (delete cascade),
because the owner can be a pending inbox row as well as a visible message.
`seq` is its message sequence or inbox `enqueued_seq`. One row can name the same
blob more than once while retaining only one reference record per hash. Media
occurrence positions must therefore come from message/part JSON.
[Reference schema](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/sql.ts#L197-L215),
[owner and reference recording](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L411-L453).

Rewrite synchronizes stale references; cancellation, revert and session deletion
release them. Releasing a reference deletes that blob when no references remain.
Blob availability is consequently time-sensitive even when transcript bytes
were previously observed. Capture raw bytes and metadata in the same SQLite
snapshot as the referring content. Other file URIs remain references whose bytes
require a separately declared capture policy; original bytes cannot be recovered
from an unavailable path or ordinary URL string alone.
[Collection/release](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L455-L507).

Tool results can also depend on retained files outside SQLite. The generic output
bounder defaults to 2,000 lines / 50 KiB of UTF-8 text, saves joined full text
under `<data-root>/tool-output/tool_<identifier>`, and returns bounded content
with a marker plus `metadata:{truncated:true,outputPath:<path>}`. Existing
`metadata.truncated` bypasses this generic bounding, leaving producer-specific
capture rules in charge. File content items retain their relative placement.
The retained-file cleanup uses a seven-day retention duration and runs hourly.
Capture a named full-output file as an explicit dependency when available, while
preserving the exact bounded transcript content as a separate body. Missing
retained files should leave the transcript and original output path intact.
[Limits and retention](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/tool-output.ts#L13-L18),
[bounding and saved output](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/tool-output.ts#L55-L125),
[cleanup schedule](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/tool-output.ts#L138-L145),
[bounding before transcript publication](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/runner/step.ts#L119-L127).

Assistant/revert snapshot IDs are also references. Native filesystem snapshots
use a Git repository under
`<data-root>/snapshot/<project-ID>/<fast-hash-of-real-worktree>` and can be seeded
from the source repository. Preserve snapshot IDs and file lists in transcript
metadata. Reproducing referenced filesystem states requires a separately captured
snapshot repository and any objects it depends on; ordinary transcript rows
carry the references.
[Snapshot repository location](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/snapshot.ts#L89-L103).

Ordinary native reads replace user media refs with inline base64, and tool refs
with data URIs. A missing user blob becomes base64 text
`[media unavailable: <name> (<mime>)]` (or MIME alone) and `mime:"text/plain"`;
a missing tool blob becomes a text result with the same placeholder. Ref-aware
reads also transform missing or unreferenced blobs to placeholders. They add
`bytes`, and non-null dimensions, to surviving user file refs. The experimental
media route only serves blobs with a remaining ref. Capturing raw rows before
these transformations preserves whether bytes were missing and the original
reference; capturing an API/export requires recording that it is a transformed
view.
[Placeholder and inline transformations](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L49-L99),
[ref-aware read and metadata](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/session/blob.ts#L156-L245).

Synthetic attachment example: these are complete stored file shapes, presented
in a documentation array. `SGk=` encodes the bytes `Hi`; the all-zero hash is a
synthetic unresolved reference and has no claimed blob dependency in this example.

```json
[
  {"data": "SGk=", "mime": "text/plain", "source": {"type": "inline"}, "name": "hi.txt"},
  {"uri": "media:0000000000000000000000000000000000000000000000000000000000000000", "mime": "image/png", "source": {"type": "inline"}, "name": "missing.png"}
]
```

## Parser/storage implications for this project

These are proposed capture boundaries, not OpenCode's own storage contracts:

- Identify a source database/profile and an epoch (replacement/reset), then scope
  native session/message/part/ref keys under that identity. Retain all raw row
  columns and JSON before compatibility normalization.
- Acquire a consistent read-only SQLite snapshot. Native runtime opens SQLite in
  WAL mode where supported; copying a live main file alone can omit committed
  WAL content. Avoid starting the native runtime to read a source, because its
  database layer applies migrations on acquisition.
- Reconcile complete session inventories, including archives and children. Use
  dirty-session indicators to reduce work while retaining periodic complete
  reconciliation. A per-aggregate sequence may help detect activity, but max
  message sequence misses updates. Counter loss/reset, imports, table-layout
  changes, media backfills and direct updates require source reconciliation.
- Publish observations, extracted content references and a checkpoint together
  only after a successful declared-scope capture. An interrupted/partial scan
  cannot establish deletion. Record missing dependencies explicitly.
- Keep native occurrence metadata (IDs, seq/ordinal, timestamp, status, usage,
  parent/fork/revert/compaction relationships, media path/name/MIME) outside a
  shared content CAS. Equal text, argument JSON or binary bodies can share
  reusable content while distinct calls, message revisions and copied history
  retain their identities and relationships.
- Define a versioned JSON canonicalization policy before hashing structured tool
  arguments/results. Preserve exact text/binary bytes and opaque replay payloads.
  A decoded logical transcript and original JSON lexical serialization are
  different fidelity levels; retain raw JSON text if lexical fidelity is promised.

[Native WAL/migration acquisition](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/database/database.ts#L33-L54),
[aggregate sequence reservation](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/src/bus.ts#L24-L48).
The event log is implementation context for these mutable projections. Durable
conversation capture does not require retaining every execution transition.

## Existing test evidence reviewed

The pinned projector tests exercise media hydration/placeholders and inline/blob
storage, media release and fork retention, interleaved parts with out-of-order
tool completion, mixed rows, replacement, legacy upgrade and flag changes. The
archive test checks both states in discovery. These were read as evidence and
were not executed during this documentation task.
[Media tests](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/test/session-projector.test.ts#L1121-L1383),
[reference lifecycle and ordered parts](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/test/session-projector.test.ts#L1565-L1820),
[mixed and flag behavior](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/test/session-projector.test.ts#L2450-L2600),
[archive listing](https://github.com/anomalyco/opencode/blob/c33eaa170ed5093c08f1684862a695aae036dcda/packages/core/test/session-archive.test.ts#L58-L70).

# Ingestion and storage design assessment

**Assessment: 2026-11-07.** This is a candidate direction, informed by the current
application handle and a source review of OpenCode at
`f460ccefcb936d4e4747b5feb3c21604d3000c8e` on its `dev` branch. It describes
observed source behavior and proposed changes separately. The proposed content
CAS, native observations and generic graph below are not implemented contracts.
Reassess these boundaries against new formats and public behavior fixtures.

The [harness history reference](harness-history/README.md) holds the detailed
per-harness native format reviews and their pinned upstream evidence.

## Requirements and terms

Near-term harnesses are Codex, Claude Code, Pi and Hugging Face Tau, and OpenCode.
The application currently imports Codex/Claude. Tau means `huggingface/tau`, the
Python implementation; it has its own native entry format despite Pi-like content.

Content addressing should deduplicate reusable content. A timestamped record's
identity and a text/tool/media body's identity have separate jobs. Keep native
occurrence IDs, times, usage and relationships outside shared content bodies.
Preserve unknown fields and record variants so interpretation can improve later.

The shared relationship language is nodes and edges. Edge kinds distinguish
parent history, containment, delegation, tool-result linkage, compaction and
native relationships. Ancestry rules apply to relevant edge kinds; the complete
graph need not have the same constraints as a session's parent tree.

## Transcript acquisition

The ingestion target is the durable conversation: user input, assistant responses,
reasoning/content blocks, tool calls/results, session metadata and relationships.
Read OpenCode's transcript tables consistently and preserve content already captured
when upstream sources later disappear.

The reviewed native representation includes:

- `session_v2`: session identity, project/parent/fork metadata and archive state.
- `session_message`: ordered messages with native metadata and inline JSON content.
- `session_part`: ordered assistant blocks where content is split out. Native reads
  also support inline and mixed layouts, so acquisition must preserve their order.
- `blob` and reference rows: media that may be externalized from message content.
- Instruction/compaction metadata needed to interpret the retained conversation.

Message and tool rows can change as a response completes. Incremental ingestion
must revisit changed sessions and reconcile current content. Use a consistent
read-only SQLite snapshot, a source-appropriate dirty token and periodic inventory
reconciliation. Capture media dependencies with the transcript. A failed or partial
read preserves the previous captured data and committed checkpoint.

For OpenCode, plan to admit **settled transcript messages** by default. Follow the
[native export eligibility rule](harness-history/opencode/README.md#acquisition-surfaces-and-their-fidelity):
an assistant needs `time.completed`; a shell or compaction must have left `running`.
Keep terminal failures and their output. Load split/inline parts and dependencies
from the same read snapshot as the message. Pending inbox rows and transient
execution events are outside this policy. An unfinished message is pending work,
not deletion evidence; revisit its session even when message `seq` is unchanged.
The settlement fixture starts with `msg_split` unfinished and completes it at
the same sequence.

The existing aggregate counter is a possible change-detection optimization; its
use should be tested against the adapter's snapshot reconciliation contract.
A maximum message ID or message creation sequence alone misses later row updates.
Migrated sessions, database replacement and schema changes require reconciliation.

We are building a history/query tool over conversation transcripts. Capturing
internal execution transitions and stream fragments is outside the current design.

## Source-wide metadata and ingestion ownership

**Clarification: 2026-11-07.** A harness input is a source store with discovery,
metadata, transcript and dependency surfaces. A single transcript file is one
possible surface. Source-wide metadata can determine selected history, names,
archive state, memberships and dependency locations. Acquire that evidence rather
than trying to infer all of it from conversation text or filenames.

The reviewed harnesses have different arrangements:

| Harness          | Relevant source-wide / associated surfaces                                                                                                             |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Codex            | Shared ThreadStore/StoredThread model, state SQLite metadata, separate paginated history SQLite, name index, active/archived plain/compressed rollouts |
| OpenCode         | Database-wide sessions, projects/workspaces, transcript rows and media dependencies                                                                    |
| Hugging Face Tau | Project/global session indexes and archive indexes alongside JSONL transcripts                                                                         |
| Claude Code      | Project/session discovery, transcript metadata, child sidecars and configuration; closed-writer limitations are documented                             |
| Pi               | Filesystem discovery and per-file headers/metadata entries; a separate global thread database is not required                                          |

These observations come from the pinned [harness references](harness-history/README.md).
Represent the common responsibilities while keeping each source's authority and
coverage rules local to its adapter. A missing optional index, an unavailable
database and an authoritative complete empty catalog have different meanings.

The intended ownership is:

| Owner                                               | Responsibility                                                                                                                                               |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Harness acquisition modules in `llm-history-ingest` | Read-only, bounded discovery/catalog snapshots; native paths and database selection; transcript/dependency acquisition plans and source-specific checkpoints |
| Pure parsing modules in the same harness            | Decode captured rows/JSON/bytes; preserve extras and field presence; interpret content, metadata and relationships deterministically                         |
| Shared capture mechanics                            | Bounded file/decompression reads, complete-record offsets and representation changes, used by acquisition workflows                                          |
| Application workflow                                | Sequence acquisition, decoding and dependency resolution; report coverage and coordinate retries                                                             |
| Storage owners                                      | Retain native observations, shared content and graph/query projections; publish related writes and checkpoints atomically                                    |

Pure lineage interpretation can operate on already captured metadata and records.
Fetching referenced files, opening SQLite and decompressing input are acquisition
effects. The same ingestion crate can own both kinds of modules with an explicit
effect boundary. Further crates should follow demonstrated ownership needs.

Current implementation has bounded Codex thread/project SQLite acquisition in
`ingest::codex::source`, catalog discovery in `ingest::catalog`, and native metadata
writers in store/harnesses. Storage still coordinates portions of ingestion; the
application-owned workflow above is candidate evolution. The current metadata DTOs
and latest-state catalog tables do not yet implement the proposed retained native
observation and general graph model.

For Codex, reuse the source's catalog and resolution semantics: the reviewed source
resolves a live writer, then selected SQLite rollout, then permitted filesystem
fallbacks. A known paginated thread with a missing selected path stays unresolved;
choosing another physical rollout could expose superseded history. A read-only
external importer captures the persisted catalog/header evidence and validates
its capture boundaries; it does not require access to Codex's in-process writer.
[Storage surfaces](harness-history/codex/storage-surfaces.md),
[readers and lifecycle](harness-history/codex/readers-and-lifecycle.md).

The capture unit can include source-wide metadata plus several physical transcript
segments. Preserve snapshot scope, completeness and observation time, and validate
selected-pointer/header/physical-revision agreement where available. Concurrent
changes can require another acquisition pass. Captured ancestor prefixes remain
dependencies of child history even when their upstream files become archived or
unavailable.

## Proposed hybrid representation

The [schema diagrams](schema/README.md) compare the current tables with a
review candidate for this design. The candidate makes native documents and
content references the replay input, with original bytes for unsafe records
or explicit archival captures. The current byte-exact export contract remains
in effect while we review that choice.

The following are responsibilities, not a prescribed table count:

| Responsibility           | Stored information                                                                                                                                                               |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Content CAS              | Reusable logical bytes and their content IDs; physical compression stays independent.                                                                                            |
| Sources and captures     | Profile/harness/source-store identity, source epoch, catalog or transcript role, schema/adapter version, snapshot scope, observation time and coverage.                          |
| Checkpoints              | Source-specific committed dirty tokens, file offsets and snapshot positions.                                                                                                     |
| Native observations      | Source-wide catalog rows/sidecars and transcript records, native table/record type/key, contextual fields, content references and unknown JSON variants.                         |
| Content references       | Native record/path/part position, content kind/representation and CAS ID.                                                                                                        |
| Nodes and edges          | Source-store, project, family/thread and occurrence identities; relationships derived from catalog and transcript observations, retaining native kinds and extension attributes. |
| Query/search projections | Purpose-built relational views, FTS documents and vector/chunk records over selected node/content revisions.                                                                     |

Adapter-owned native tables are reasonable when their shapes differ. Pi and Tau
need distinct entry decoders. Shared content handlers can be narrower than shared
native record types. Known fields deserve types; open namespaced variants/extras
retain fields we have not yet interpreted.

### Codex source-store mapping

This mapping is a candidate implementation of the responsibilities above:

| Native evidence                                                        | Retained representation / projection                                                                                                           |
| ---------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| A configured Codex source store/home and optional separate SQLite home | Profile/source-store identity and native locations/configuration; scoped native ID namespace                                                   |
| SQLite thread/project/section/spawn rows and name-index records        | Native metadata observations, typed fields plus extras; shared thread/project nodes and relationships where meaningful                         |
| Selected `threads.rollout_path`, validated header and history mode     | An observation of the selected physical history revision; replaceable current-selection projection                                             |
| Logical thread ID versus physical rollout ID                           | Stable thread identity with associated physical sources/revisions; several files can belong to one thread                                      |
| Plain/zstd siblings and archive/unarchive paths                        | Physical representations/locations and source availability of retained history; neither transition alone creates a new conversation occurrence |
| `history_base` and its ordinal/decoded-byte cutoffs                    | Bounded physical-prefix dependency edges, captured once and referenced by descendant history                                                   |
| Family ID, fork origin and spawn parent                                | Separate family-membership, logical-fork and execution-parent edges                                                                            |
| Copied prefix boundary and compaction replacement                      | Inherited-context/context-checkpoint evidence; transcript and effective-context projections use it without duplicating activity                |
| Body/media fields                                                      | Shared content CAS references; native IDs, times and memberships stay outside reusable content                                                 |

Catalog observations can produce nodes/edges without producing a message body.
Catalogs therefore do not need to be stuffed into a synthetic conversation event
or hashed together with its content. Retain previous project names/memberships and
selected-history observations while separately updating current source state.
This accommodates the historical retention requirement within the hybrid model.

For a transcript containing a tool call and result:

```text
assistant message -> text block -> CAS A
                  -> tool call -> arguments CAS B
                         |
                      result -> output CAS C
```

Native IDs, times, status and usage remain on the relevant records/nodes. Another
occurrence can reference the same content while retaining its own relationships.
The adapter supplies native ordering and relationship meaning. Native records can
contribute several graph nodes/edges, and several records can describe one message
or tool operation.

Preserve absence, explicit null and empty values where the native format gives
them different meaning. Store unknown relationship types and unresolved native
references. Avoid defaulting unfamiliar records into a misleading known category.

Content extraction should yield metadata/native structure plus explicit content
references. Use exact text or binary bytes directly. Structured JSON equality
requires a defined, versioned serialization policy. Signatures and opaque replay
data remain associated with the native blocks they describe. A content hash must
never collapse independent executions or usage occurrences.

Unknown records can remain native JSON variants until an adapter understands
their content boundaries. Improved decoders can revisit retained observations.
Lossless logical reconstruction and exact original JSON formatting are separate
fidelity choices; define the promised fidelity for each capture format.

Snapshot absence can retract a current native observation only after a successful
complete scan of its declared scope. Acquisition errors leave previous state and
checkpoint intact. Upstream deletion records availability/visibility changes;
retained CAS content follows our archive retention policy. Collection follows
references from retained observations, not just the active graph projection.

Publish content references, observations/projections and their checkpoint in one
store transaction. Acquisition and inference stay outside that transaction where
possible. Source-specific acquisition reads files/SQLite/APIs; deterministic native
decoders own content extraction. This preserves the current goal of keeping
database operations out of pure parsers and public APIs independent of bindings.

## Incremental work selection and publication

The [extraction plan](ingestion-extraction-plan.md) maps this candidate workflow
onto current code, callers, dependency changes and acceptance gates.

**Candidate workflow clarified 2026-11-07.** Pure decoders map captured native
records into metadata, content candidates and relationship evidence. Source adapters
interpret their own change markers and checkpoint semantics. The application owns
the workflow that uses those decisions to acquire and publish work.

| Phase                | Inputs and result                                                                                                      | Owner                                                                        |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Observe and plan     | Bounded native catalog/source observations plus saved checkpoint and decoder revision produce explicit work decisions  | Application coordinates; harness adapter interprets source-specific evidence |
| Acquire and decode   | Selected suffixes/ranges/snapshots become bounded prepared batches, including content identities and captured metadata | Acquisition/capture modules and pure harness decoders                        |
| Validate and publish | Expected destination checkpoint, captured observations, content/relationships and next checkpoint commit together      | Storage transaction, coordinated by application                              |

Keep source reads, decompression and parsing outside destination write transactions.
Prepared batches may use bounded buffers or temporary spooling. Publication checks
that the destination checkpoint still matches the one used to plan work; concurrent
updates produce superseded/deferred work rather than an in-call replan loop. Freeze
the finite workset and per-source cutoffs during observation. Post-cutoff growth is
normal and waits for another invocation. Source acquisition validates the required
captured prefix and reports incomplete coverage. A watermark advances through
successfully committed work. Partial batches can commit independently when their
source-specific checkpoint and relationship dependencies are complete. Shared
invocation budgets bound every phase; unchanged no-progress work is deferred under
cooldown/resume policy. Expected source churn is an informational deferral rather
than an instruction for the invoking harness to run the tool again immediately.

Work decisions should distinguish unchanged inputs, metadata-only updates, append
suffixes, selected ancestor ranges, mutable-snapshot reconciliation and replay from
retained captures after a decoder change. A metadata edit, relocation, archive
transition or representation change need not decode all conversation content again.
Capture and semantic interpretation revisions have distinct invalidation reasons.

The change-evidence ladder is source-specific:

1. Establish source identity/epoch, physical history identity and format/checkpoint
   compatibility. Database replacement or a changed selected rollout can invalidate
   a previously valid cursor without changing logical thread identity.
2. Use native complete-coverage change markers/cursors where their behavior has
   been established. Keep mutable-snapshot dirty tokens separate from append positions.
3. Preserve the current size/mtime no-read fast path for unchanged files. Database
   file observations include WAL state when relevant; their filesystem timestamps
   are dirty hints rather than logical row cursors.
4. Use xxh64 fingerprints as continuity validation and fallback evidence: existing
   file head/boundary windows, a stable cursor anchor, or captured candidate native
   rows when no reliable mutation cursor exists.
5. Reconcile the affected bounded scope when evidence disagrees or is insufficient.
   A complete inventory establishes absence; an incomplete one preserves prior state.

A high-water mark is a source position with its scope/epoch and validation evidence.
For files that can be a committed decoded-byte offset plus ordinal; for a declared
append-only database stream it can be an ordered native cursor plus anchor. A maximum
row ID or creation timestamp misses in-place updates/deletions in mutable stores.
Such stores need an update-aware cursor or snapshot reconciliation. Window hashes
are an append-continuity heuristic; explicit repair/reconciliation handles changes
outside those windows according to the adapter's coverage contract.

For Codex, native catalog data supplies thread identity, selected rollout and
dependency metadata. A thread timestamp or a lagging SQLite projection offset is
insufficient by itself to prove that the underlying rollout has no new records.
Validate selected physical files and complete decoded-record boundaries. Keep
plain/zstd/archive paths as representations/locations, and freeze inherited ranges
against physical rollout identity rather than only the logical thread ID.

### Hash ownership

| Algorithm/evidence                       | Purpose                                                                       |
| ---------------------------------------- | ----------------------------------------------------------------------------- |
| Native markers and cheap source metadata | Select likely changed scopes and skip unchanged sources                       |
| xxh64                                    | Fast fingerprints for continuity/change detection and fallback cursor anchors |
| BLAKE3 of extracted body bytes           | Shared reusable-content CAS identity                                          |
| Existing BLAKE3 raw-record identity      | Separate exact-capture identity retained by the current implementation        |

The same xxh64 value does not establish content identity. Content extraction yields
the exact text/binary bytes or an explicitly versioned structured encoding; the
ingestion/content preparation path computes BLAKE3 once as that content is first
captured. Native IDs, time and occurrence metadata remain outside shared body hashes.
Reopen, query and retained replay reuse persisted identities. Verification remains
explicit rather than adding full-source rehashing to routine update paths.

### Implemented workflow

The [application ingestion workflow](ingestion-extraction-plan.md) plans and
prepares Codex and Claude input before publication. Shared values carry expected
checkpoints, capture bounds, records and decoder state. Storage validates the
checkpoint and commits the prepared batch atomically. The JSONL capture uses
the implemented [schema 1 archive](schema/current.dbml). Mutable row snapshots,
prefix binding and graph projections are candidate extensions.

## Behavior fixtures that should shape the implementation

- Equal bodies with different timestamps/IDs share CAS objects and retain distinct
  nodes/occurrences, native relationships and usage.
- A tool updates in place after its message creation sequence; the next import
  captures its new state and reuses unchanged argument content.
- Inline, split and mixed assistant parts produce equivalent public content/order.
- Fork inheritance reserves sequence positions and remaps identities; it preserves
  the complete copied history even if the parent later disappears upstream.
- Revert/delete removes upstream rows and media references; our retained observations
  still resolve their captured content. Archive flags remain separate from deletion.
- Unknown table columns, record variants and native fields survive capture and can
  be interpreted by a newer decoder without live sources.
- Failed/partial snapshots, counter resets and concurrent updates
  preserve consistent checkpoints and report their actual coverage.
- Unchanged source metadata causes no content read/decompression; changed native
  metadata with unchanged selected transcript performs metadata-only work.
- Appends validate their continuity evidence and process only complete suffixes;
  conflicting fingerprints or source epochs select scoped reconciliation.
- A failed publication advances no checkpoint; concurrent planners cannot publish
  against an obsolete destination checkpoint. Retried work remains idempotent.
- A decoder change reinterprets retained captures without re-reading live sources
  or re-hashing already stored content.

## Pinned source references

- [Projection and deletion operations](https://github.com/anomalyco/opencode/blob/f460ccefcb936d4e4747b5feb3c21604d3000c8e/packages/core/src/session/projector.ts)
- [Message/part/blob schema](https://github.com/anomalyco/opencode/blob/f460ccefcb936d4e4747b5feb3c21604d3000c8e/packages/core/src/session/sql.ts)
- [Media collection](https://github.com/anomalyco/opencode/blob/f460ccefcb936d4e4747b5feb3c21604d3000c8e/packages/core/src/session/blob.ts)

Validation was a source and existing-test review. This checkout has no installed
dependencies or Bun executable on the task PATH; upstream tests were not executed.
No personal database or live service was inspected or changed.

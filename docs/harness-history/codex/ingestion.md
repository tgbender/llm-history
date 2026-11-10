# Codex capture and content extraction

Reviewed **2026-11-07** at `openai/codex` `main` commit
`2bc8e7f5459b084afa482c7aac3f56fef36286b3`. These are adapter design
implications derived from the pinned source in [the format review](README.md),
not an implemented storage contract in llm-history-rs.

Codex acquisition includes its source-wide thread catalog and selected-rollout
metadata as well as transcript records. The [storage specification](storage-surfaces.md)
documents its shared ThreadStore/StoredThread/ThreadMetadata types and their files.
The [source-store/model mapping](../../ingestion-storage.md#source-wide-metadata-and-ingestion-ownership)
defines the candidate acquisition/parsing/storage boundary and maps this evidence
into retained observations, content references and graph relationships.

## A complete durable capture

1. Record effective home, source profile/store identity, source epoch, adapter
   revision and coverage. Inventory active and archived plain/compressed rollouts,
   titles and current-thread pointer evidence. Keep file path as provenance;
   thread identity comes from decoded metadata plus physical filename identity.
2. Read through a bounded snapshot of the selected file. Preserve complete
   original records with file byte boundaries and unknown JSON fields. Treat an
   incomplete final line as pending acquisition; retain an error/coverage record
   for malformed interior records rather than silently claiming a complete scan.
3. Follow `history_base` recursively to its **rollout** ID and exact exclusive
   ordinal/decoded-byte boundary. Capture referenced ancestors, including
   archived physical files, before publishing a complete logical conversation.
   Missing ancestors create unresolved history edges with explicit coverage.
4. Capture binary dependencies that are present and needed to interpret message
   and tool content: inline image/audio bytes, local artifacts explicitly
   referenced by content, image-generation results and opaque provider blobs.
   `file_id` and remote URL values are references; retain them with provider/model
   scope and fetch/availability status. A transcript alone may not include the
   referenced binary. Do not substitute the current contents of a local path for
   the historic attachment bytes without evidence they are the same artifact.
5. Reconcile source revisions when compression, migration, revert, archive or
   replacement changes path/size/header/boundaries. Track selected-history revision
   independently from the raw capture. A successful complete inventory can mark
   availability changes; a failed or incomplete inventory cannot retract prior
   observations.

Source justification: [lineage bounds][lineage], [compression][compression],
[revert pointer switch][revert], [migration publication][migration],
[media types][media]. The acquisition policy itself is a recommendation.

An append checkpoint needs the physical file/revision identity and an exact
complete-record position. Paginated ordinal alone is insufficient across revert
or migration. Byte offsets into `.jsonl.zst` compressed bytes do not equal native
`HistoryPosition.end_byte_offset`: native offsets address the decoded JSONL.
[Ordinal state][ordinal], [revert][revert].

## Content CAS and occurrence metadata

| Native material | Reusable content candidate | Retained occurrence/native structure |
| --- | --- | --- |
| Message text block | Exact text bytes; keep block kind outside or in a versioned typed body | Thread/rollout/ordinal, role, block index, message/turn IDs, phase, origin, timestamps |
| Function call | Exact argument string, or a separately versioned canonical JSON body if parsing succeeds | Tool name/namespace, native call ID, response ID, parent turn, argument representation/parse status |
| Custom tool call | Exact freeform input bytes | Name/namespace, call ID, routing and ordering |
| Tool output | Exact string, or ordered structured blocks with media references | Call linkage, occurrence time, output position, native status/error/truncation markers |
| Reasoning/compaction | Each plaintext summary/body; each opaque encrypted blob as its own byte content | Native kind, producer compatibility hash, window relationships, completeness and replay association |
| Binary content | Captured decoded bytes plus encoding/MIME metadata | Original URL/file ID, block index, provider scope, availability and association with raw record |
| Usage | Usually native occurrence/checkpoint data | Response/turn/root attribution and cumulative scope; preserve unknown counters |
| Retained context | Original text/answers as content, with a reference back to original evidence when known | Acceptance order, inherited/local origin, completeness, revision and role |
| Tool/instruction catalogs | Versioned structured JSON bodies or exact serialized source value | Effective position/scope and relationship to context revision |
| Unknown item | Raw source record for future decoding | Native variant, provenance and undecoded state |

A raw JSONL line or whole file is useful as a lossless **capture object**. Its
hash is not the reusable conversation body: timestamps, ordinals, native IDs,
usage and host metadata make equal prose produce distinct raw objects. Keep the
raw capture reference and separately extract content references from semantic
boundaries. Distinguish exact original serialization from logical JSON
reconstruction; define canonical JSON policy explicitly if structured values
are deduplicated.

An equal argument/output/text body can share CAS storage across two executions,
while each call/result/utterance remains an independent native occurrence. Two
response items with the same text can have different roles or sources. A copied
fork prefix and a checkpoint replacement can refer to existing bodies without
asserting that their contents happened again. `event_msg` and `response_item`
can represent the same user/assistant contribution in different native forms;
keep their evidence before constructing a single semantic projection.
[Native envelope/metadata][history], [persistence policy][policy].

Signatures/encrypted content/passthrough objects are replay-sensitive. Keep their
association with the exact native block and representation even when readable
text is indexed separately. Preserve unknown provider fields rather than using
Codex's typed `Other` fallback as an archival representation. The source itself
has serialization-only and deserialization-only fields, so a typed round-trip
can discard evidence. [Response model definitions][media].

## Relationships and useful fixtures

Use separate edges for root-session containment, delegation, logical fork,
physical inherited prefix, tool-call/result, compaction replacement, context
window succession and observed rollback/revert. Scope every native lookup by
store/profile and native identity kind. A UUID-looking ID is not sufficient to
merge nodes from independent homes.

Good adapter fixtures derived from the review include:

- Identical body with two timestamps/native IDs: one content object, two
  occurrences; different usage remains attached to each occurrence.
- Two simultaneous tool calls with results returned in reverse order; a missing
  or absent call ID remains unresolved instead of pairing by adjacency.
- Same stable thread with two physical rollout IDs after revert; current pointer
  changes and captured excluded turns remain available as earlier history.
- Child with `history_base` into a compressed archived ancestor; capture resolves
  the exact decoded prefix and survives later source deletion.
- `compacted` checkpoint with aligned metadata and unchanged original input IDs;
  a summary and retained-context evidence remain distinct.
- Legacy missing ordinal, numeric old window ID, `agent_type` alias, decimal
  `create_time`, and unknown response variant; raw values survive normalization.
- File truncation/rewrite during migration and incomplete final append: no
  checkpoint advancement beyond complete captured coverage.

[history]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/lib.rs#L55-L405
[lineage]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/rollout_lineage.rs#L279-L597
[compression]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/compression.rs#L63-L110
[revert]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/revert_thread.rs#L15-L151
[migration]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/thread-store/src/local/rollout_migration.rs#L1-L84
[ordinal]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/ordinal.rs#L16-L131
[media]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/models.rs#L883-L1395
[policy]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/policy.rs#L16-L116

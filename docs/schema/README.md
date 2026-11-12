# Storage models for review

**Snapshot: 2026-11-09.** These are diagrams for comparing the implemented
storage with a candidate design. The repo SQL remains authoritative for the
current database. See the [DBML syntax reference](https://dbml.dbdiagram.io/docs/)
for ways to read or render the files.

| Diagram                                                   | Scope                                                                                                                                                                                                  |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [Implemented tables](current.dbml)                        | Application tables for source checkpoints, native retention, CAS, harness metadata, query projections and vectors. Views, macros, generated FTS tables and staging are omitted.                        |
| [Native content candidate](native-content-candidate.dbml) | Source tracking, retained native evidence, body CAS, graph, and two example Codex projections. Public query, FTS and vector tables are outside this comparison; they remain purpose-built projections. |

The [ingestion workflow](../ingestion-extraction-plan.md) prepares source data
before opening the write transaction and preserves byte-exact exports. A newer
build upgrades a database from 0.1.0 or later when it opens it. The candidate diagram addresses
future storage needs. Its keys and references describe logical
invariants; physical DuckDB types, constraints and indexes still need review.

## Implemented retention

The archive factors known UTF-8 strings out of a native JSONL document when the
archive codec can reproduce the original string token exactly. The compressed
layout retains every other byte. A `content_parts` row identifies the JSON path
and the byte position of a `null` placeholder in that layout. Reassembly restores
the original bytes, including line endings, field order and escape spelling.
Small strings, unfamiliar records and unsafe-to-split values remain inline.

`native_documents.hash` is the existing whole-line identity. Identical complete
lines share one layout; different lines can share the same body via
`content_objects.id`, a BLAKE3 digest of the unescaped UTF-8 bytes. Each occurrence
keeps its own `native_observations.raw_id` and original decoded positions. Batch
publication inserts new objects, document layouts, observations and the checkpoint
in one transaction. Replay and export share a reconstruction path. Existing
whole-line archives continue to export byte for byte.

Cursor CLI snapshots use the same capture and replay transaction with the
`sqlite-rows-jsonl` encoding. The archived stream contains the selected root,
its message subtrees and leaves, and session metadata. The SQLite file, WAL,
and sidecar stamps decide whether to open a read transaction. A failed read
publishes neither a replacement projection nor a checkpoint. Raw export
returns the retained row stream; byte comparison to the live SQLite file does
not apply.

The candidate below remains the direction for capture of mutable SQLite rows,
catalog history, physical prefix binding and graph projections. Those features
are separate from the implemented JSONL archive.

## What changes in the candidate

| Today                                                                                                                        | Candidate                                                                                                                                                        |
| ---------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sources` combines path, availability, change evidence and committed parser state.                                           | A source has locations and per-stream checkpoints. A capture pins the observed epoch, scope and cutoff.                                                          |
| Native observations retain original JSONL records. Known body leaves can share CAS objects; `events` holds interpreted text. | The same retention model extends to typed SQLite rows and related binary objects.                                                                                |
| Catalog and thread tables maintain latest metadata.                                                                          | Catalog rows are observations with their own checkpoint. A rename or archive change can publish without reading a transcript.                                    |
| Session and event relations use specialized projections.                                                                     | Profile-scoped nodes and evidence-backed edges distinguish forks, inherited prefixes, delegation and memberships. An observation can explain zero or more nodes. |

For ordinary understood records, the native document plus the content parts is
the replay input. A JSON pointer identifies each externalized leaf, and its
representation tells reconstruction how to restore the value. Paths are unique
and non-overlapping within a document; a placeholder can be an ordinary JSON
null because only the side table identifies externalized leaves. The document
itself retains array order. BLAKE3 hashes
the exact extracted body bytes, independently of source, role, order or time.
For example, two tool calls with identical argument strings share a body object
while retaining separate call IDs, results and usage.

Keep unfamiliar valid JSON whole and mark it `raw_only`. If the capture encoding
cannot preserve a record safely (for example, duplicate keys would become
ambiguous), retain original bytes within the capture budget. Preserve missing
versus null, array order, precise numbers, tool arguments stored as strings and
opaque values that depend on representation. SQLite row capture needs storage
classes and blob dependencies, including the distinction between SQL NULL and
TEXT containing JSON `null`. Typed harness tables are replaceable interpretations;
a later decoder can revisit observations without the live source. Unknown fields
on a decoded record stay in the native document. Decoder changes or explicit
replay can retry interpretation; ordinary refresh does not retry unchanged
raw-only observations.

Revisions and absence are scoped to source epoch, stream and locator. Re-observing
an unchanged row need not create a new revision. An update from A to B and back
to A is still three observations, even if A reuses body objects. An OpenCode
part's locator is the table plus `(message_id, ordinal)`; changing its state
adds an observation at the same locator, regardless of message `seq`. Within
one stream and epoch, publication order determines the latest observation for
a locator. The publisher checks the expected checkpoint so an older prepared
snapshot cannot supersede a newer publication. A complete
inventory of a declared scope, or explicit native deletion evidence, can establish
absence; failed and partial reads preserve previous evidence. A Codex archive
move changes location/availability.

`retained_prefixes` records which physical source, epoch and decoded boundary
actually satisfied a child's native prefix claim. Store this binding once the
parent range is retained; a future projection rebuild uses that same epoch even
if the live parent was rewritten or disappeared. A missing prefix remains an
unresolved native claim with incomplete coverage. The `inherits_prefix` graph
edge is a projection of the binding. Reconstruction may change JSONL line
lengths, so the offsets always refer to the captured source.

## First implementation slice

Use the [synthetic fixtures](../../fixtures/native-history/README.md) to prove
the retained representation before adding graph and harness projections:

1. **Reconstruct one record from storage.** Externalize known UTF-8 text and
   exact tool argument strings. Rebuild the object with its original fields,
   arrays and leaf values using only the retained document and CAS. The same
   body in two Codex occurrences should have one content ID and two distinct
   observation IDs. Run the recovery cases through the same boundary: retain
   unfamiliar valid JSON whole, preserve unsafe completed records as bytes, and
   leave the incomplete tail beyond the committed checkpoint.
2. **Publish one mutable snapshot.** Use OpenCode's seed and settlement SQL.
   Capture native SQLite storage classes; tag known JSON-bearing TEXT columns
   before extracting leaves from their nested documents. Leave ordinary or
   opaque TEXT as exact strings. Use settled transcript admission by default:
   the seed's running assistant and its parts remain pending, and the same
   message and part locators become capturable after settlement without a higher
   message `seq`. A previously retained settled revision remains readable if a
   message is later rewritten. Only a previously retained row can gain absence
   evidence, after a complete scan of its scope. Commit observations, content
   references and actual progress together.
3. **Pin a physical prefix.** Resolve the compressed Codex parent at original
   ordinal 9 and decoded byte 2405, then remove the source files in the test.
   Replay the bound prefix and exclude the later parent record. The native
   projection cursor is deliberately behind and must not cap the retained range.

The JSONL retention slice is implemented. Use isolated test stores to prove
the mutable snapshot and prefix slices before wiring them into the publisher.
Add projections for actual query needs. Pi/Tau branches and Claude's tool and
sidecar links give us fixtures for those projections.

## Follow-up decisions

1. **Additional formats.** JSONL export still produces exact captured bytes.
   Define reconstruction and export separately for typed SQLite rows and any
   future representations whose native byte serialization is unavailable.
2. **Prove the encoding and replay rules.** Use synthetic cases for unsafe JSON,
   unknown variants, a mutable SQLite part, and a Codex child that inherits a
   compressed or deleted parent. Check reconstruction, original boundary lookup,
   stable occurrence IDs, and replay under an upgraded decoder. Pin how catalog
   revisions and relationship retractions are ordered at publication.
3. **Measure the tradeoff.** Compare compressed bytes, ingestion time and replay
   time with the existing whole-record archive on synthetic copied forks and
   compaction snapshots. Include content reference and projection costs. Sharing a
   body once should save space across timestamped copies; the net result depends
   on how many whole records the current archive already deduplicates and whether
   small leaves cost more to reference than to retain inline.

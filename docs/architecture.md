# Architecture assessment

**Snapshot: 2026-11-07, application-handle baseline `647f0b3`.** This document
describes the code at that point and the reasoning behind possible next changes.
The crate/module sketch below is a candidate direction, not a prescribed final
layout. The application and its evidence should determine future boundaries.

Code health needs periodic assessment: whether the crates, modules and interfaces
still serve the application, whether the tests exercise the right contracts, and
whether responsibilities have accumulated in the wrong places. Revisit this
assessment during normal project reviews and when the triggers below occur.
Keep the observation date and actual implementation distinct from proposals.

Selected invariants have executable guards in
[`tools/architecture`](../tools/architecture). Those reviewed rules have a
different role from this descriptive snapshot. Boundary changes can update the
rules with a rationale in the same change. Tests deliberately do not enforce a
crate count, directory tree, module size limit, or a fixed set of harnesses.

The [storage diagrams](schema/README.md) compare the implemented schema with the
candidate native observation and content model.

## Implemented shape

The public entry point is `llm-history-app::History`. The CLI has its own crate
and uses the handle for database operations. The handle retains configuration;
individual operations acquire connections and release them. The public behavior
contract is described in [application-api.md](application-api.md).

Production workspace dependencies at the application-handle baseline:

```text
cli    -> app, core, search, models
app    -> core, store, search, models
search -> core, store, models
store  -> core
core   -> no other workspace crate
models -> no other workspace crate
```

CLI additionally uses store as a dev-dependency for existing internal tests.
The production graph is acyclic. Cargo already rejects production dependency
cycles; the architecture suite additionally checks ownership directions that
could be violated without creating a cycle.

| Crate  | Responsibilities observed in this snapshot                                                               | Pressure to reassess                                                                         |
| ------ | -------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| core   | Canonical/fact types, both native parsers/state, capture, discovery, time and text helpers               | Provider and acquisition changes share a large implementation with domain policy.            |
| store  | DuckDB/archive persistence, batching, schema/views, capture/replay coordination, native catalogs/threads | Persistence also knows native formats and source-discovery details.                          |
| search | Candidate retrieval, fusion, answer selection, excerpts, durable indexing worker                         | Query algorithms and job lifecycle evolve for different reasons.                             |
| models | Model contracts/profiles, artifacts, HTTP, token handling and managed processes                          | Several independent implementation lifecycles share lib.rs.                                  |
| app    | History handle, initial DTOs, operations, refresh/index initialization                                   | Public types still include implementation reexports; some operations still access Store.con. |
| cli    | Argument/default mapping, formatting, model assembly, detached worker spawning                           | Some application/runtime policy remains frontend-owned.                                      |

Fourteen of fifteen store integration-test files directly use its connection.
Those cases include valuable fault injection and storage invariants; internal
tests can move alongside private storage code as the connection becomes private.
The seven public application acceptance tests and CLI contract runner provide a
separate behavior boundary, using synthetic native transcripts and temporary
databases. Their fixtures retain exact raw bytes and test identities through
append, reopen and replay.

## Candidate ownership and layout

### Ingestion and shared-type extraction added 2026-11-07

After the application-handle snapshot above, the workspace gained
`llm-history-ingest` in `crates/ingest`. This is one ingestion adapter crate with
`codex`, `claude_code`, `opencode`, `pi` and `tau` modules. Shared JSON and
field-presence support lives inside the crate and depends on Serde/serde_json.
`llm-history-types` now owns shared `Harness`, `Profile`, `Usage`, `Fact` and
session/record/context/token metadata values. Consumers import those types
directly; compatibility exports from core/app were removed.

The implemented production edges after this extraction are:

```text
types  -> no workspace dependencies
core   -> types
ingest -> types, core
store  -> types, core, ingest
search -> types, core, store, models
app    -> types, core, store, search, models
cli    -> app, types, core, search, models, store (archive-file API only)
models -> no workspace dependencies
```

The existing CLI core/search/models edges remain the recorded legacy ownership
exceptions. Store still coordinates ingestion; app workflow extraction remains
future work. Search now consumes shared query and progress values from types.

The subsequent API cleanup on the same date removed all 26 app foreign exports,
the two dependency-type exports from ingest and the application wildcard export.
Shared query/message/receipt/resource values moved from store/search into types.
Application-owned contracts now have explicit `api::{query,ingestion,sql,search}`
modules. There are no foreign public re-exports or wildcard public exports in the
workspace, and source checks enforce both rules.

Archive verification/listing/restore are defined only in store's public archives
module; the CLI can call those three file operations. Source checks still reject
Store access and database workflow bypasses. Filesystem database-path resolution
is defined once in `core::paths`. Four test-only Store forwarding methods and the
paired single-file import helper were removed; tests use the current bounded
query APIs and ordinary profile update. Private generation keys remain in storage
rows rather than shared response values. Small test batches are explicit config.

These modules implement open native envelope/row readers, with public synthetic
examples and unknown-data tests. They preserve unknown fields/variants, original
JSON values and missing/null distinctions; typed known-field access fails
explicitly if its shape changes. OpenCode receives native column representations
from acquisition and owns no database connection. Codex and Claude semantic
decoding, native session/context metadata and continuation state now live in
their harness modules. Shared record sequencing/checkpoint structure lives in
ingest; shared capture/discovery/time/title/search policy remains in core.
Live capture and retained replay use the same decoder.
Parser revision 6 and serialized checkpoint fields are retained. Complete fact
and checkpoint fixtures frozen before extraction protect both uninterrupted and
resumed decoding, alongside public application/CLI contracts. Content-CAS and
new harness semantic decoding remain future work.
See [the crate layout and migration boundary](../crates/ingest/README.md).

Workspace guards keep this integration family independent of storage, models and
CLI dependencies. Source guards protect its pure JSON/record/row/decode/state
modules, shared type effects and foreign/wildcard public exports. Future
source-specific discovery/acquisition can have separate modules without putting
I/O in decoding. Split crates when actual dependency or independent-use needs justify
that boundary; each native format currently has its own module.

### Storage family extraction added 2026-11-07

The [storage family](../crates/store/README.md) now has three actual packages under
`crates/store`: engine, history and harnesses. Engine owns the native build/sys
binding, configured connections, stable locks and borrowed write transactions.
The existing store package and implementation live in history. Harnesses owns
Codex native thread persistence and retained catalog snapshots/assignments.

Current production storage edges:

```text
history  -> engine, harnesses, types, core
harnesses -> engine, types
engine   -> types
```

Codex SQLite thread/project reads moved to ingest/source; native JSON catalog
decoding moved to the respective harness modules. Table writers accept captured
values and a borrowed engine transaction, perform no acquisition and never commit
independently. The history owner coordinates native writes with shared projections
and checkpoints. Late-failure/retry and committed-reopen tests exercise that seam.

The connection is now private, all five app SQL bypasses have typed storage
operations, and all six source exceptions were removed. Storage SQL fault-injection
tests moved inside history; public app/CLI tests remain external. Shared schema
assets live beside the history schema runner; native DDL belongs to harness storage.

**Retained archive and workflow update, 2026-11-09.** Live Codex and Claude
ingestion records native observations and capture ranges and shares selected text
through content CAS. Harness decoders select leaves; storage owns body hashes,
atomic publication and archive reconstruction used by replay and export. See the
[implemented tables](schema/current.dbml). `app/src/ingestion` coordinates
update, catalog refresh and retained replay. Storage accepts prepared values and
has no production dependency on ingestion. Graph projections remain a candidate.
Ingest owns transcript directory selection, source keys and resume requirements.
The app sequences those operations and supplies stored state on request.
Live preparation releases the database between checkpoint reads and publication.
The storage engine provides a separate ingestion lock, shared by profile changes
and restore, to serialize source state across those connection lifetimes.
Foreground inference also runs outside the database. Session, project and usage
queries return shared typed rows directly to the app; CLI aliases remain at the
frontend.

### Candidate growth

The following responsibilities are likely useful as the application grows.
Extract cohesive modules first; create a crate when a dependency or independent
interface boundary justifies it. Keep a single active implementation during moves
and update imports to the owner directly.

```text
types/      shared identity, session/record metadata, fact and usage values
core/       shared interpretation, visibility/search-document and output policy
capture/    source descriptors, bounded JSONL/compressed capture, checkpoints
ingest/     ingestion adapters, shared decoding, per-harness modules
store/      connection, schema/, archive/, write/, query/, embedding/
models/     contracts/profiles, runtime/, artifacts/, embedding/, rerank
search/     request, pipeline, candidates, ranking, answers, excerpt
app/        history, api/, operations/, jobs/embedding/, runtime assembly
cli/        arguments, commands/, output/, detached-process entry points
```

`capture` remains an extraction candidate. `ingest` owns decoding, transcript
inventory, source planning and native reads. The app coordinates the workflow.
Individual candidate filenames/modules should exist when their responsibilities
warrant them.

Proposed ownership principles:

- Canonical value types and shared interpretation policy are independent of native
  formats, database engines and frontend syntax.
- Pure adapter decoding accepts native records and adapter-owned state and emits
  canonical updates. Native JSON/SQLite catalog acquisition belongs to source
  modules, separated from pure decoding. Discovery uses source-specific knowledge;
  bounded capture uses format-specific mechanics.
- Application operations sequence discovery, capture, decoding, persistence and
  replay. Store operations own transaction invariants: raw records, projections
  and checkpoints commit atomically. DuckDB SQL stays with storage.
- Search owns candidate composition, ranking and answer/excerpt selection. Durable
  embedding jobs have queue, budget, resume and publication responsibilities that
  can live under app/jobs, using models and typed storage operations.
- Public requests/responses describe caller operations. Canonical reusable values
  can be intentionally reexported; physical database rows and parser state need
  their own implementation ownership.
- CLI code translates flags/defaults, renders compatibility output and manages
  frontend process entry points. Runtime assembly can move behind application
  configuration while model process/HTTP implementations remain in models.

Useful local patterns are small lib.rs export maps, capability-oriented API
modules, private implementation modules, explicit sibling imports and traits at
actual substitution boundaries. Concrete types/modules are sufficient for most
internal collaboration. The tests below protect selected ownership edges as this
layout evolves.

## Harness and data-model evolution

The [harness history reference](harness-history/README.md) maintains per-harness
durable format reviews with dates, checkout pins and public upstream references.
Use those native-format findings when evolving adapters and behavioral fixtures.

The [ingestion/storage assessment](ingestion-storage.md) records the 2026-11-07
OpenCode source review and a candidate content-CAS/native-observation/node-and-edge
model. OpenCode ingestion targets its durable conversation transcript: messages,
content parts, tool calls/results, media and session relationships. Snapshot
reconciliation handles native row updates. The assessment separates verified
upstream behavior from proposed storage responsibilities and fixtures.

The near-term support targets, as of 2026-11-07, are Codex, Claude Code,
OpenCode and pi/tau. The current importer implements Codex and Claude only.
Use these targets to assess the next adapter boundary: OpenCode's native
database-backed session/message/content-part state requires consistent snapshot
acquisition; Pi's JSONL session tree requires preserving entry parent links,
compaction entries and custom extension records. Tau here means Hugging Face's
Python implementation (`huggingface/tau`): it uses its own entry envelope, numeric
timestamps and separate active/archive indexes, with Pi-style nested content.
Keep native entry decoders and harness identity explicit; assess shared content
extraction separately. Supported versions and historical layouts need native fixtures.

The next extraction should separate source acquisition from decoding retained
inputs. Keep content-addressed payload storage and ordered occurrence/revision
metadata shared; let adapters preserve their native input shape. Synthetic
acceptance scenarios should cover capture, incremental change, source disappearance
and replay for each supported adapter. For tree-based sessions, define branch
selection separately from file order and from the set of retained searchable events.

The intended seams support changing native formats and richer shared concepts.
Codex/Claude native decoding is now separated by module. The existing shared
checkpoint layout and one parser revision remain, along with a mostly
text-oriented fact model; richer content, per-harness revisions and the proposed
graph/CAS output require further implementation.

1. **Adapter-local change.** A harness can change record layout, paths, native
   metadata, usage accounting or checkpoint format. Its adapter owns that change
   and its native fixtures. Canonical concepts already represented should flow
   through shared query/storage behavior without new provider branches there.
2. **Source acquisition change.** A database-backed or mutable-document harness
   may need a new capture mechanism. Source descriptors should distinguish exact
   retained material from a reconstructed export and preserve native locators.
3. **Shared model change.** Ordered content blocks, revisions/branches, richer
   relationships or independent role/purpose/run-mode facets may require an
   intentional canonical contract/projection change. Assess actual harness
   evidence, migration/replay and query compatibility together.

Preserve raw material and native labels/evidence so interpretation can be improved
later. Represent missing/unknown canonical values explicitly; namespaced extension
metadata can retain adapter-specific facts without making every one a universal
column. Shared concepts should acquire explicit shared types when they become
useful to queries. Existing default classifications need their own compatibility
decisions as this evolves.

Physical schema, canonical projection and per-adapter decoder/checkpoint versions
are distinct evolution concerns. Separating their ownership is a candidate change;
it should preserve resumability and per-source rebuild diagnostics. A small
compiled-adapter registry is sufficient initially. Runtime plugin loading remains
an option to evaluate against a concrete use case.

Architecture guards should protect dependency direction and ownership, while
allowing new harnesses, native events, metadata fields, canonical variants and
supported formats. Behavioral fixtures should protect documented semantics and
unknown-data handling, with explicit expectations for intentional model changes.

## Executable guardrails and limits

Run `mise run architecture`. It uses a project-local, locked Python environment
under `tools/architecture/.venv`, pytest and the Rust Tree-sitter grammar. `mise run
check` and `mise run test` include the suite. Lock refresh uses
`UV_EXCLUDE_NEWER=3d uv lock --project tools/architecture`; normal checks use frozen
sync/run. Direct Python testing dependencies and uv/Python tools are pinned.

The current checks cover:

- The production Cargo graph, including build/optional/target-specific workspace
  dependencies, is acyclic. Dev-dependency edges are assessed separately.
- Workspace dependency directions and selected database/network/CLI dependency
  ownership. Cargo metadata resolves renamed dependencies.
- CLI production Rust code can call only the three archive-file functions in
  store::archives; Store/database workflow access remains forbidden.
- Library code does not select stdout/stderr, exit the process, or print through
  terminal macros. Callers may provide writers.
- Pure native decoding/checkpoint modules, shared types and application contract
  modules avoid explicit I/O/backend/frontend dependencies. API modules avoid
  explicit references to application workflows/handles.
- Foreign values/functions are imported from their owning crate. Public
  cross-crate re-exports, direct foreign type aliases and wildcard public exports
  are rejected.
- Engine connection types stay inside the storage family. Native repositories
  cannot open source/engine databases or depend on ingestion implementations.
- Store connections are private, with no source-level legacy exceptions remaining.
- Application ingestion workflows leave concrete harness selection rules to
  ingest. The source guard catches direct references to harness variants there.

The exception registry is [`policy.toml`](../tools/architecture/policy.toml).
It records three existing CLI bypass dependencies, with reasons. The previous
connection-access/public-field exceptions have been removed. Current history
workflow placement and future model/schema changes still need assessment.

The source rules use syntax trees, grouped/renamed import paths, and a conservative
`test=false` view of cfg predicates. Unknown platform/feature branches remain
checked. Comments, strings and test-only modules do not count as production code.
The scanner follows production module declarations from Cargo's runtime targets.

These are source-level architectural checks, not Rust semantic resolution or
macro expansion. Wildcard imports, helper indirection and generated code can need
additional rules or compiler-supported visibility constraints. Syntax parse and
module-discovery failures fail the suite instead of silently reducing coverage.
The selected known pure scopes need reassessment when they move or new adapters
are introduced.

Module relationships need ownership-aware rules. A facade reexport and a shared
value-type reference differ from a lower layer invoking an application workflow.
The suite currently guards selected one-way module boundaries; it does not impose
universal acyclicity on every Rust module/reference.

## Tests at appropriate levels

| Owner            | Main test responsibilities                                                                |
| ---------------- | ----------------------------------------------------------------------------------------- |
| Canonical domain | Pure values, time/identity, visibility and searchable-content policy                      |
| Native adapters  | Synthetic native records, canonical output, unknown events, checkpoint resume             |
| Acquisition      | Temporary inputs, partial lines, rewrite/append, compression, bounds, exact bytes         |
| Storage          | Real temporary DuckDB, transactional failure/recovery, schema/views, stale publication    |
| Search           | Pure ranking/answer/excerpt cases and focused store integrations                          |
| Models           | Fake local HTTP/processes, normalization, tokenization, deadlines and cleanup             |
| Application      | Public lifecycle scenarios, real persistence and deterministic model fakes                |
| CLI              | Flags/defaults, text/JSON shape, stderr/exit/output behavior and selected smoke scenarios |

Architecture tests complement these behavioral tests. Shared fixture scenarios can
exercise full import versus append/restart, compression and archive-only replay.
Internal corruption/SQL staging tests belong near private storage implementation;
the documented history SQL views remain a public contract.

## Reassessment triggers

Revisit ownership when adding a harness, capture format, frontend or model runtime;
when one feature repeatedly requires native branches across unrelated crates; when
public callers need implementation access; when exceptions accumulate; when model
changes become awkward to represent; or when locks/performance/test setup reveal
that an operation lives at the wrong boundary.

For each assessment, record what is implemented, what changed in the application's
needs, which boundaries still help, and which should move/merge/split. Update the
date, candidate layout, relevant executable rules and tests together. A passing
suite is evidence for its selected invariants; periodic design judgment remains
part of maintaining the application.

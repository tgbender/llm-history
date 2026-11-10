# Durable harness history formats

These notes describe the native conversation history saved by each harness:
user input, assistant responses, content blocks, tool calls/results, usage,
session metadata, relationships and referenced artifacts. Their purpose is to
keep importer work grounded in what is actually persisted.

**Last review round: 2026-11-07.** Each harness folder records the exact upstream
checkout revision and relevant package/format version evidence. Those pins define the
scope of its findings. Reassess the notes when upstream storage, serialization,
session reconstruction or artifact handling changes.

| Harness | Format notes | Reviewed checkout |
| --- | --- | --- |
| Codex | [Codex](codex/README.md) | [openai/codex `2bc8e7f545`](https://github.com/openai/codex/tree/2bc8e7f5459b084afa482c7aac3f56fef36286b3), `main` |
| Claude Code | [Claude Code](claude-code/README.md) | [public repository `0ed94698aa`](https://github.com/anthropics/claude-code/tree/0ed94698aa0aeee471465fc9c867c2e8434c4d68) and [Python Agent SDK `2209f2c134`](https://github.com/anthropics/claude-agent-sdk-python/tree/2209f2c134fee914c0f75733b631a5fc238ca846), both `main` |
| OpenCode | [OpenCode](opencode/README.md) | [anomalyco/opencode `c33eaa170e`](https://github.com/anomalyco/opencode/tree/c33eaa170ed5093c08f1684862a695aae036dcda), `dev` |
| Pi | [Pi](pi/README.md) | [earendil-works/pi `a4ccccbd64`](https://github.com/earendil-works/pi/tree/a4ccccbd64d4ccea1644f5a07bb94ef09075b219), `main` |
| Tau | [Hugging Face Tau](tau/README.md) | [huggingface/tau `47610d3ed7`](https://github.com/huggingface/tau/tree/47610d3ed71deb70b07443a8641588ffc9c8ffc6), `main` |
| Cursor | [Cursor](cursor/README.md) | CLI build `2026.10.19-6bbf2c4`; reference readers and fixture evidence pinned in the Cursor notes |

Tau here is Hugging Face's Python implementation. Pi and Tau share concepts and
some message shapes while using different durable entry formats.

The [synthetic native fixture corpus](../../fixtures/native-history/README.md)
contains test inputs for the reviewed harnesses, with notes on which pinned readers
and schemas were used to validate them.

## Reading the specifications

Native storage, reconstructed conversation, provider request context and public
SDK output are distinct representations. Each review identifies which one its
fields and examples describe. Synthetic examples illustrate the reviewed shape;
they do not establish an exhaustive grammar or a permanent upstream contract.

Evidence is strongest where the persisted writer, reader and relevant tests agree
at the pinned revision. Published documentation and SDK readers provide narrower
evidence for closed implementations. Claude Code's review marks confirmed reader
behavior, published contracts, inference and unresolved writer details explicitly.

The documents cover discovery and native data locations, identities and ordering,
field spelling and timestamp units, content/tool structures, relationships,
compaction/fork/archive/deletion behavior, compatibility and capture dependencies.
References point to upstream repositories and documentation; source claims should
use revision-pinned permalinks. Native data-path conventions are part of the format
description and use upstream defaults/environment variables rather than personal
checkout paths.

## Implications for llm-history

The importer supports Codex, Claude Code, and Cursor JSONL and CLI chat stores.
The other folders document support targets.

The [ingestion adapter crate](../../crates/ingest/README.md) groups per-harness
native envelope readers and shared JSON/presence support. Full normalized decoding
and acquisition migrate separately; the format notes remain the source reference
for that work.

Keep reusable content identity separate from native occurrence identity. A text
body or image can be shared in CAS while occurrences retain native IDs, timestamps,
role/authorship, usage, ordering and relationship metadata. The adapter must define
the extraction boundary and preserve unknown variants/fields for later decoding.

Use the [ingestion/storage assessment](../ingestion-storage.md) for the candidate
graph/CAS/native-record design and the [architecture assessment](../architecture.md)
for crate ownership. The harness folders describe upstream facts; design proposals
belong in those separate assessments.

## Keeping the review useful

For a subsequent review:

1. Record the review date, upstream commit, branch/tag and relevant package/format
   version. Update source permalinks together with factual changes.
2. Trace serialization, read/reconstruction and lifecycle operations. Check existing
   native fixtures/tests, including malformed, legacy and unknown shapes.
3. Distinguish field absence, explicit null, empty values and generated defaults;
   distinguish timestamps at each nesting level and scope every native identifier.
4. Check dependencies beyond the main transcript: indexes, database rows, blobs,
   sidecars, archives and parent histories.
5. Record remaining uncertainty and which checks actually ran. Add synthetic importer
   acceptance fixtures when a finding becomes an implemented behavior contract.

Review only public source and synthetic data for these documents. Personal
conversation content and machine-specific source-checkout references do not belong
in the specification.

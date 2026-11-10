# Historical Codex history formats and handling

Reviewed **2026-11-07**. This document records saved-format evidence from four
explicit historical release snapshots, alongside the main source review at
`2bc8e7f5459b084afa482c7aac3f56fef36286b3`. Older snapshots were read directly
from existing Git objects without switching or modifying the checkout.

The importer target is historical and present saved data. A current writer's
filters do not define which records deserve acquisition or preservation. Reader
acceptance, migration-only handling and original historical writer output are
separate evidence. Version strings are hints; record shape and file context drive
decoding. Migrated files and inherited prefixes can mix older and newer records.

[historical-snapshots.json](historical-snapshots.json) inventories exact enum
declarations, serde attributes, selected payload declarations, output codecs and
writer/reader/policy functions at the snapshots below. It is a set of concrete
observations, not a claim about every release between them. No introduction or
removal interval is inferred from a single sampled tag.

## Inspected snapshots

| Release | Commit / commit date | Stored family | Outer / response / event Rust variant counts |
| --- | --- | --- | --- |
| 0.20.0 | `45aed5edd139c38132de044596fdd55e47236657`, 2025-08-08 | Unwrapped metadata, bare responses and state lines | No RolloutItem enum / 6 / 25 |
| 0.50.0 | `b4123b7b1db22a3c0a8b133a23c7b30a477d7b65`, 2025-10-25 | Timestamped type/payload envelope, no ordinal | 5 / 9 / 40 |
| 0.100.0 | `8272f9a71ee7364d8de992418103628f8a17eb6f`, 2026-02-12 | Timestamped envelope; replacement checkpoints and ghost snapshots | 5 / 11 / 70 |
| 0.140.0 | `6506579001c322927a3e4bd440563267a7ac6c1f`, 2026-06-15 | Timestamped envelope; communication records and numeric compaction window ID | 6 / 16 / 76 |

Event counts inventory the protocol enum, not the number of event kinds written
by that snapshot. Rust variant names and wire names also differ; inspect the
recorded serde attributes. Renamed Rust variants can retain old wire tags.
[0.20 writer][bare], [0.50 declarations][v50], [0.100 declarations][v100],
[0.140 declarations][v140].

## Unwrapped/bare history

At 0.20.0, the writer stores:

1. An unwrapped first-line `SessionMeta`: `id`, creation `timestamp`, nullable
   `instructions`, and optional flattened Git metadata.
2. Bare typed response objects such as `{"type":"message",...}`,
   `function_call`, `function_call_output`, `reasoning` and `local_shell_call`.
3. State updates with `{"record_type":"state",...}`. The reviewed state snapshot
   type is empty; retain unknown state fields rather than assuming it stays empty.

There is no outer `payload`, per-record timestamp or ordinal. The original resume
reader parses the first header directly, skips whitespace/malformed later lines,
recognizes state records, and decodes supported bare response items. Its writer
appends compact JSON plus LF and flushes. The current canonical enveloped parser
rejects this family; that rejection is a parser-format mismatch rather than proof
of invalid saved history. [Original writer/reader][bare].

Handling requirements:

- Recognize a session header from file context and its fields; distinguish a bare
  response item's `type` from the later outer record discriminator.
- Preserve exact original bytes, source order/byte coordinates, state and extras.
- Keep per-item time absent when the file supplies none. Session creation time
  does not establish every response's occurrence time.
- Extract bare text/tool/media content through the same content handlers used
  after modern envelope decoding. Keep the native dialect/shape with the record.
- Report incomplete or unrecognized material without advancing beyond committed
  capture coverage. Historical source readers' permissive skipping is evidence
  about those readers, not an archival deletion policy.

The authored [bare-history example](examples/historical-bare.jsonl) illustrates
this family. Its fields follow the inspected declarations; it is synthetic data.

The old tool-output payload struct uses internal `content` and `success`, but its
custom serializer writes `output` as a plain string and drops `success`. The
historical codec and tests establish this even where nearby comments describe a
different API input shape. Preserve the actual wire output and retain any native
status evidence separately. [0.20 output codec][bare-models].

## Earlier enveloped histories

At 0.50.0, `RolloutLine` has timestamp plus flattened outer item; its outer enum
contains session metadata, response, compaction, turn context and event. No ordinal
field exists in the inspected declaration. The writer timestamps each record in
UTC. Session metadata includes string `instructions`, while later snapshots carry
`base_instructions`. Turn context uses `sandbox_policy` and older approval/model
configuration. Its compaction record contains only `message`.
[Declarations][v50], [writer][v50-writer].

Preserve old instruction fields and summary-only checkpoints. A summary alone does
not provide a complete model replay snapshot. The authored
[enveloped-history example](examples/historical-enveloped.jsonl) captures this
minimal family. Physical capture position can supply an importer-owned ordering
coordinate while the native ordinal remains explicitly absent.

At 0.100.0, `CompactedItem` additionally supports optional `replacement_history`.
`ResponseItem::GhostSnapshot { ghost_commit }` and alias `compaction_summary` for
encrypted compaction appear in the source. The snapshot's actual persistence
policy accepts GhostSnapshot. Later Codex removes ghost snapshots from bulk replay,
including aligned checkpoint entries; retain the original native data when our
archive imports it. [Declarations][v100], [historical persistence][v100-policy],
[later stripping][ghost-strip].

At 0.140.0, `inter_agent_communication` is a durable outer kind, and compaction's
`window_id` is `Option<u64>`. At the main review pin, the custom reader accepts
numeric legacy `window_id` as a window number, while string IDs retain distinct
identity semantics. Preserve the original number/string distinction before any
normalization. Parent/fork/agent metadata is richer in this later snapshot, but
older missing fields must remain unknown rather than being inferred from content
similarity. [Declarations][v140], [window compatibility][windows].

## Handling matrix rather than release-specific parser copies

[record-catalog.json](record-catalog.json) records compatibility shapes with
recognition, interpretation and preservation rules: historical instructions,
aliases, omitted IDs/ordinals, retained-delivery wrappers, old permissions,
dynamic-tool forms, checkpoint sidecars and numeric windows. The
[lifecycle catalog](lifecycle-catalog.json) records the reader-specific decisions:
accept, normalize, skip, reject, migrate or preserve as unresolved.

A single harness module can dispatch these few structural families and reuse
content/relationship decoders. Add a shape handler when evidence requires it;
avoid making a copy of the parser for every release. New fields and unfamiliar
variants retain their native JSON and source coordinates until a later decoder
can interpret them. No present enum is an allowlist for all history.

The four historical snapshots are checks against present-day assumptions. Source
is available for further targeted investigations when a newly encountered shape
does not fit these families; a future format cannot be specified in advance.

## Implementation status

These are format/handling specifications. The current llm-history Rust adapter
preserves captured source records and has open JSON readers, but its semantic
Codex decoder is oriented around enveloped records. Bare 0.20-style history,
modern bounded inheritance, subagent inherited-prefix projection and some older
event aliases still need explicit semantic support and synthetic acceptance tests.
Do not interpret inventory coverage as implementation of every documented handler.

[bare]: https://github.com/openai/codex/blob/45aed5edd139c38132de044596fdd55e47236657/codex-rs/core/src/rollout.rs
[bare-models]: https://github.com/openai/codex/blob/45aed5edd139c38132de044596fdd55e47236657/codex-rs/core/src/models.rs#L200-L241
[v50]: https://github.com/openai/codex/blob/b4123b7b1db22a3c0a8b133a23c7b30a477d7b65/codex-rs/protocol/src/protocol.rs#L935-L1011
[v50-writer]: https://github.com/openai/codex/blob/b4123b7b1db22a3c0a8b133a23c7b30a477d7b65/codex-rs/core/src/rollout/recorder.rs#L395-L417
[v100]: https://github.com/openai/codex/blob/8272f9a71ee7364d8de992418103628f8a17eb6f/codex-rs/protocol/src/protocol.rs#L1828-L1940
[v100-policy]: https://github.com/openai/codex/blob/8272f9a71ee7364d8de992418103628f8a17eb6f/codex-rs/core/src/rollout/policy.rs
[ghost-strip]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/recorder.rs#L1263-L1345
[v140]: https://github.com/openai/codex/blob/6506579001c322927a3e4bd440563267a7ac6c1f/codex-rs/protocol/src/protocol.rs#L2839-L3070
[windows]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/rollout_payload.rs#L430-L468

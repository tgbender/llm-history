# Codex JSONL format at the reviewed revision

Review date and exact source pin: **2026-11-07**, `openai/codex` `main` at
`2bc8e7f5459b084afa482c7aac3f56fef36286b3`.

This is the concise overview of the enveloped family. See
[historical formats](historical-formats.md) for earlier unwrapped/bare histories,
and [the detailed wire specification](wire-format.md) plus
[record catalog](record-catalog.json) for aliases, custom codecs and exact fields.
Writer filtering at this pin does not limit which saved records an importer retains.

This inventory describes serialized names, not Rust enum spelling. It covers
all outer rollout variants and the conversation payload families at this pin.
Nested policy/provider extensions remain open JSON. Optional below means an
observed codec supports absence; a producer may write null, omit the value or
emit only some variants. Preserve that distinction in captured JSON.

## Outer record

The writer serializes one compact JSON object plus LF per append and flushes it.
Fields are `timestamp` (string), optional `ordinal` (unsigned 64-bit), flattened
`type` (snake_case) and `payload`; `response_item` can additionally have sibling
`metadata`. `payload.item` is not the native response-item layout. Legacy records
can lack ordinal. The canonical reader removes timestamp/ordinal before decoding
the item so nested decimal JSON numbers retain precision.
[Writer](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/recorder.rs#L2064-L2110),
[parser](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/lib.rs#L47-L83).

| Outer `type` | `payload` and interpretation |
| --- | --- |
| `session_meta` | Flattened session metadata plus optional `git`; see below. |
| `dynamic_tools_updated` | `{thread_id,tools}`: complete subsequent tool catalog; empty tools clears it. |
| `response_item` | Raw typed model-history item; optional harness `metadata` sits beside payload. |
| `inter_agent_communication` | `author`, `recipient`, `other_recipients`, `content`, `trigger_turn`; optional `id`, `encrypted_content`, passthrough. Routing/IDs and content belong together. |
| `inter_agent_communication_metadata` | `{trigger_turn}` boolean. |
| `compacted` | `{message,...}` checkpoint; replacement items/metadata, retained families, origins, windows and usage can accompany it. |
| `turn_context` | Turn baseline configuration/context; see below. |
| `token_usage_record` | Per-response usage plus turn/thread cumulative counters and attribution IDs. |
| `world_state` | `{full,state}`; full snapshot or patch of model-visible comparison baseline. |
| `retained_context` | Tagged host facts: `accepted_file_audio`, `verified_answer`, or a decoder-supported `delivered_assistant_message`. The latter's current serializer emits a compatibility `response_item` instead. |
| `security_risk_score` | `scores` map, optional `call_id`, `action` JSON, `sampled_at` UTC timestamp; keep as harness metadata. |
| `event_msg` | Nested `type`-tagged protocol event; selected durable events complement response items. |
| `realtime_item` | Sparse model-invisible presentation facts, persisted in paginated mode. |

The full table is backed by the actual [wire enum and transforms][wire].
[Communication fields](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L981-L1000),
[security score](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/security_risk.rs#L8-L25).
Persistence is separately filtered by [policy][policy]. Known `ResponseItem`
variants are persisted except `compaction_trigger` and `other`; realtime
persistence depends on history mode. Some command/MCP event outputs have 64 KiB
persistence caps. A decoder-supported variant is therefore not evidence that all
instances appear in every rollout.

## Session and context metadata

The [session type and deserializer][session] define these serialized fields:

- Identity/family: `session_id`, `id`, `forked_from_id?`, `attached_thread?`,
  `forked_from_ordinal_exclusive?`, `parent_thread_id?`.
- Creator scope: `account_scoped?`, `creator_user_id?`, `creator_account_id?`.
- Creation/runtime: `timestamp`, `cwd`, `runtime_workspace_roots?`, `originator`,
  `cli_version`, `source`, `thread_source?`, `model_provider`.
- Agent: `agent_nickname?`, `agent_role?` (accepts historical `agent_type`),
  `agent_path?`, `multi_agent_version?`.
- Context: `base_instructions`, `dynamic_tools?`, `selected_capability_roots?`,
  `memory_mode?`, `history_mode`, `history_base?`,
  `subagent_history_start_ordinal?`, `context_window?`.
- `git?` is flattened beside those session fields by `SessionMetaLine`.

`history_base` has `thread_id`, `end_ordinal_exclusive`, `end_byte_offset`;
`context_window` has `window_id` (UUIDv7). `history_mode` is `legacy` or
`paginated`. `source` is a serde union, so preserve its actual JSON value;
subagent source metadata can be structured rather than a role string. The
writer initializes metadata from runtime inputs; default implementations describe
fallbacks rather than a universal population guarantee. [Metadata writer][meta-writer].

The [turn-context type and compatibility serializer][context] define
`turn_id?`, `root_turn_id?`, `direct_only_tool_namespaces?`, `disabled_plugin_ids?`,
`cwd`, `approval_policy`, `approvals_reviewer?`, `permission_profile`,
`active_permission_profile?`, `model`, `comp_hash?`, `collaboration_mode?`,
`multi_agent_version?`, `realtime_active?`, `cyber_access_program?`, `effort?`,
`summary?`. The wire adds legacy `sandbox_policy` and sometimes
`file_system_sandbox_policy`; the reader reconstructs newer permission profiles
from older fields. Preserve wire values before normalization. Empty routing/plugin
lists and missing fields have different restoration meanings.

## Response items and content

[ResponseItem][models] uses another snake_case `type` discriminator. Optional
shared fields include `id`, `encrypted_content`, `status`, and
`internal_chat_message_metadata_passthrough` where the individual variant
supports them. An item ID and a tool `call_id` are separate identifiers.

| Payload `type` | Core fields / content shape |
| --- | --- |
| `message` | `role`, ordered `content` array; optional `phase` (`commentary`, `partial_answer`, `final_answer`), ID/status/encrypted content/passthrough. |
| `additional_tools` | `role`, `tools` array of opaque JSON declarations, optional `id`. |
| `agent_message` | `author`, `recipient`, ordered content blocks (`input_text` or `encrypted_content`), optional ID/passthrough. |
| `reasoning` | `summary` array (`summary_text` blocks with `text`), optional `content` array (`reasoning_text` or `text`), `encrypted_content` nullable, optional ID/passthrough. Plaintext content serialization is conditional. |
| `function_call` | `name`, optional `namespace`, `arguments` **string containing JSON**, `call_id`; optional ID/status/encrypted content/`encrypted_function_args` array/passthrough. |
| `function_call_output` | `output`: string or structured content array; optional `call_id`, `id`, `name`, `namespace`, passthrough. |
| `custom_tool_call` | `call_id`, `name`, optional `namespace`, freeform `input` string; optional ID/status/encrypted content/passthrough. |
| `custom_tool_call_output` | `call_id`, `output` with the same string-or-array codec; optional ID/name/passthrough as defined by the type. |
| `local_shell_call` | Legacy `id?`, `call_id` nullable, `status`, typed `action`, optional encrypted content/passthrough. |
| `tool_search_call` | `call_id` nullable, `execution`, `arguments` JSON value, optional ID/status/encrypted content/passthrough. |
| `tool_search_output` | Nullable `call_id`, `status`, `execution`, `tools` array; optional ID/passthrough. |
| `web_search_call` | Optional `id`, `status`, typed `action`, encrypted content/passthrough; native search item contains its own action/result semantics. |
| `image_generation_call` | `status`, optional `revised_prompt`, `result` string, optional ID/encrypted content/passthrough. |
| `compaction` | `encrypted_content` string, optional ID/passthrough; reader also accepts `compaction_summary`. |
| `configuration_update` | `reasoning` configuration at its exact position. |
| `context_compaction` | Optional ID/encrypted content/passthrough. |
| `compaction_trigger` | Request-only control, excluded from durable persistence by policy. |
| Unrecognized | Rust maps to `Other` and excludes it from new persistence. An archive decoder should retain original JSON so unfamiliar values survive. |

Message content blocks are `input_text{text}`, `output_text{text,annotations?,logprobs?}`,
`input_image{image_url|file_id,detail?}` and `input_audio{audio_url|file_id}`.
`image_url`/`audio_url` may be inline data URLs or references; preserve the source
representation and separately capture available binary dependencies.
`detail` accepts `auto`, `low`, `high`, `original`. Text annotations and logprobs
can contain arbitrary provider JSON, including empty arrays.
[Content types](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/models.rs#L883-L981).

Tool-output content arrays support `input_text`, `input_image`, `input_audio`
(with `audio_url`) and `encrypted_content`. The Rust internal
`FunctionCallOutputPayload{body,success}` serializes **directly** to string or
array: `{body:...,success:...}` is not the saved `output` shape, and internal
`success` is not serialized by this codec.
[Output codec](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/models.rs#L2286-L2488).

Link tools by scoped native `call_id`; responses and outputs need not alternate.
Parallel tool calls can precede outputs returned in completion order. An output
may be unmatched, have an absent call ID or be truncated. Keep its original
position and unresolved link. `exec`/`wait` cells and nested tools can be described
in passthrough metadata rather than additional standalone calls; retain those
fields for future interpretation. The [passthrough type][passthrough] includes
`turn_id`, fractional-second `create_time`, content kinds/metadata, `cell_id` and
executed-call inventories; some are serialization-only host fields and some are
read-only telemetry. A generic typed deserialize/reserialize cycle loses such
asymmetry.

## Harness metadata, usage and checkpoints

`response_item.metadata` has these serialized names at this pin:
`history_sequence`, `sampled_through_sequence`, `guardian_sources`,
`guardian_review_ids`, `guardian_source_order_guidance`,
`guardian_retained_omissions`, `retained_source`, `additional_context_tool_output`,
`client_authored`, **`fallback_token_limit_override`** (the Rust field is
`history_truncation_token_limit`), `delivered_assistant_message`,
`harness_authored_configuration`, `compaction_without_file_error_sources`,
`compaction_model_hash`, `user_input_order`, `compaction_output`,
`inherited_user_message`, `mcp_attribution`, `native_search_fallback_used`,
`sender_user_messages`. Many empty/default values are omitted. Preserve booleans,
bounds, IDs and completeness alongside associated bodies. [Harness definition][harness].

`token_usage_record.payload` carries `thread_id`, `turn_id`, `session_id`,
`root_turn_id`, `response_id`, `usage`, `turn_token_usage`, `thread_token_usage`.
Each usage object has signed integer token counters `input_tokens`,
`cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`,
`reasoning_output_tokens`, `total_tokens`. `usage` is a response occurrence;
turn/thread counters are cumulative checkpoints. Summing every cumulative value
would overcount. `codex_rollout_budget_units` is reader-supported provider
telemetry with serialization suppressed. Keep source JSON if it appears in
historical files. [Usage types][usage].

`compacted.payload` includes `message`, optional `replacement_history`,
`replacement_history_metadata`, `guardian_history`, `retained_context`,
`mcp_resource_origins`, `window_number`, `first_window_id`, `previous_window_id`,
`window_id`, `compaction_response_id`, `latest_token_usage_record`,
`resume_metadata`. The codec accepts a historical numeric `window_id` and
normalizes it to window number. When metadata exists, its array aligns one-to-one
with replacement items, inserting defaults where an item had none. A metadata
array without replacement history is invalid to the typed reader.
[Checkpoint codec][checkpoint].

`event_msg.payload` is the protocol's large discriminated event union, including
message/turn lifecycle, tool completion, token counts, context and rollback facts.
Durable event policy retains `item_completed`, `token_count`,
`thread_goal_updated`, `thread_rolled_back`, `turn_aborted`, `task_started`,
`task_complete` and `thread_settings_applied`. The serialized turn-lifecycle
names are **`task_started` / `task_complete`**; the reader also accepts
`turn_started` / `turn_complete`. Legacy-only message/reasoning/review/tool-end
events complement these. Paginated `item_completed` stores public `TurnItem`
objects, including plans/extension items and visible tool details that cannot
always be reconstructed from model response items alone. A transcript projection
should consume those durable items as well as response/checkpoint/context records.
[Lifecycle naming](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L1584-L1607),
[mode-specific event policy](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/policy.rs#L130-L290). User/assistant event text
may be a second representation of an already stored response item. Retain both
native observations and map their semantic relationship; content equality alone
cannot decide whether they are separate utterances.
[Protocol events](https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L1536-L1835),
[persistence policy][policy].

## Authored synthetic fixtures

[conversation.jsonl](examples/conversation.jsonl) is newly authored, contains no
captured conversation, and illustrates split message/media blocks, parallel
calls and out-of-order tool results, sibling harness metadata, encrypted
reasoning and a usage record. All IDs and bodies are illustrative. It is a
fragment: a production session starts with its own complete session metadata.

[lineage-and-compaction.jsonl](examples/lineage-and-compaction.jsonl) contains
independent illustrative fragments: a prefix reference and a checkpoint with
positional metadata. Its byte offset is a placeholder, not a resolvable file.
These examples establish JSON syntax and discussion cases; upstream execution
and semantic fixture conformance were not run.

[wire]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/rollout_payload.rs#L33-L311
[policy]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/policy.rs#L16-L116
[session]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L3440-L3623
[meta-writer]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/rollout/src/recorder.rs#L924-L1004
[context]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L3640-L3740
[models]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/models.rs#L1067-L1395
[passthrough]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/models.rs#L988-L1065
[harness]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/lib.rs#L55-L177
[usage]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/protocol/src/protocol.rs#L2524-L2573
[checkpoint]: https://github.com/openai/codex/blob/2bc8e7f5459b084afa482c7aac3f56fef36286b3/codex-rs/history/src/rollout_payload.rs#L324-L460

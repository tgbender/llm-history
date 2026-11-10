# Authored synthetic Tau examples

[example.jsonl](example.jsonl) and [example-index.jsonl](example-index.jsonl) are **authored synthetic** files. All IDs, timestamps, paths, model/provider names, usage and content are invented. The transcript uses canonical storage emission for None omission and camelCase nested aliases. The index example is a separate metadata file, not a transcript header. Optional provider diagnostics, legacy variants and images are omitted from this fixture; [format.md](format.md) specifies them.

The transcript starts with ordinary session_info/model/thinking entries. User and assistant toolCall are followed by toolResult. An initial answer remains as an abandoned sibling; a second answer branches from the same result. A label targets the abandoned answer globally. A top-level custom_message uses snake_case custom_type and seconds timestamp. Compaction retains from that custom_message, includes camelCase nested usage inside its snake_case outer fields, and is followed by a new assistant answer. A custom diagnostic/state record advances the default leaf without adding model context.

On default replay, original abandoned answer is off-path, while its bookmark is visible. Compaction context becomes summary user message, retained custom runtime message and final assistant reply; original user, call and result remain in raw storage. Summary replay creates a fresh runtime millisecond timestamp under current implementation; its stored seconds timestamp remains the original event evidence. The index title is the current display name; the earlier session_info title may differ.

For a reduced **authored synthetic legacy** compaction example (common fields included), compare these two records:

```json
{"type":"compaction","id":"legacy1","parent_id":"previous","timestamp":1794000100.25,"summary":"Earlier work","replaces_entry_ids":[]}
```

```json
{"type":"compaction","id":"legacy2","parent_id":"previous","timestamp":1794000100.25,"summary":"Earlier work"}
```

The first retains prior context and appends summary. The second, with no boundary, yields summary-only context. Their standard canonical serialization can converge by omitting empty replaces_entry_ids, which is why capture retains original field presence. These snippets are independent schema illustrations and are not additional lines to append to the full fixture.

# Cursor local history

**Reviewed 2026-11-09.** The importer supports local JSONL and the CLI
message projection described below. The desktop database is documented for
future support.

| Surface          | Native evidence                                                                                                                                                                                                                                                                                               |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Agent transcript | `projects/<workspace-slug>/agent-transcripts/` under the configured Cursor data root. The CLI and editor can both produce these files. The JSONL uses `role` and `message.content[]` with text and tool inputs; the sampled CLI files had no tool output, per-record time, or workspace path. Use line order. |
| CLI chat         | `chats/<workspace-hash>/<session-id>/store.db`, with an optional sibling `meta.json`. The sidecar can supply title, working directory, creation and update times. Keep these as session metadata.                                                                                                             |
| Desktop composer | `User/globalStorage/state.vscdb`, with workspace storage providing additional metadata. `composerHeaders` and `composerData:<id>` describe sessions; ordered headers can refer to individual `bubbleId:<id>:<bubble-id>` rows. This is a separate source from the CLI store.                                  |

Cursor documents [transcript paths and workspace roots in hooks](https://cursor.com/docs/hooks#common-schema)
and [the nested subagent transcript layout](https://prod.cursor.com/docs/release-notes#v2026-10-17).
The transcript path can be absent. [Cursor support](https://forum.cursor.com/t/accessing-the-full-agent-transcript-in-cursor/157311/5)
confirms that JSONL omits tool outputs. A workspace slug does not uniquely
identify a filesystem path.

## CLI store and fixture

SQLite schema version 1 has `meta(key, value)` and `blobs(id, data)`. The value
at metadata key `0` is hex-encoded JSON. Its `latestRootBlobId` selects the
active conversation. Blob IDs in the fixture are SHA-256 of their bytes.
Read a consistent SQLite snapshot, including committed WAL changes.

The sampled CLI build (2026.10.19-6bbf2c4) had two kinds of references in the
same root: ordered message leaves in protobuf field 1, and native turn
references in field 8. The [fixture](../../../fixtures/native-history/cursor/)
uses both. Reviewed readers also handle nested message subtrees, wrapped JSON
leaves, or a separate turn index. The fixture exercises direct references and
one native turn per root.

The selected and abandoned roots share a prompt. The active branch has two
calls to the same tool, with results in reverse call order. The abandoned
branch holds a conflicting result for one call ID. Select reachable
occurrences, link results by call ID within the conversation, and treat the
matching JSONL as another representation of the same session. The assistant
reply is inserted before its calls to make SQLite row order incorrect.

`drop-active-result.sql` leaves one message reference dangling while the root
and the other leaves stay unchanged. A reader should surface the incomplete
message projection. This mutation leaves the separate native turn graph in
place.

## Reviewed implementations

| Checkout                                                                                                                                        | Evidence used                                                                                                                                                                                                                                                                                                                                                                                                               |
| ----------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [cursor-history v0.19.0, MIT](https://github.com/S2thend/cursor-history/tree/b40612e7666b9ce7ad462e9dcaec9468e7431495)                          | [Active message leaves, tool linkage, and completeness](https://github.com/S2thend/cursor-history/blob/b40612e7666b9ce7ad462e9dcaec9468e7431495/src/core/store-stack/store-db.ts#L455-L600). Its [synthetic tests](https://github.com/S2thend/cursor-history/blob/b40612e7666b9ce7ad462e9dcaec9468e7431495/tests/unit/store-stack-store-db.test.ts) include missing leaves, nested references and out-of-band tool results. |
| [AgentsView, MIT](https://github.com/kenn-io/agentsview/tree/4d28061ee83000a587e81d0d38887e49c7eb55ae)                                          | [Read transaction and selected turn index](https://github.com/kenn-io/agentsview/blob/4d28061ee83000a587e81d0d38887e49c7eb55ae/internal/parser/cursor_store.go#L195-L308); [superseded root and WAL tests](https://github.com/kenn-io/agentsview/blob/4d28061ee83000a587e81d0d38887e49c7eb55ae/internal/parser/cursor_store_test.go#L177-L288). Its turn reader enriches text and time.                                     |
| [txcript, Apache-2.0](https://github.com/skillsynchq/txcript/tree/c60e686e7ed30e67019288eed80c4be478512d64)                                     | [Native shell-step encoding](https://github.com/skillsynchq/txcript/blob/c60e686e7ed30e67019288eed80c4be478512d64/src/harness/cursor.rs#L767-L909) and [desktop format](https://github.com/skillsynchq/txcript/blob/c60e686e7ed30e67019288eed80c4be478512d64/docs/formats/cursor-desktop.md). Its CLI normalization uses row order.                                                                                         |
| [chat-history 0.9.2, MIT](https://github.com/ay-bh/chat-history/tree/c08d1c3152704f32b5b4a0be6b6ba1e8958d2a11)                                  | [Desktop composer and bubble reads](https://github.com/ay-bh/chat-history/blob/c08d1c3152704f32b5b4a0be6b6ba1e8958d2a11/src/cursor_ide.rs) and tests. CLI SQLite-only entries carry metadata.                                                                                                                                                                                                                               |
| [Cursor session interoperability, MIT](https://github.com/xhluca/cursor-session-interoperability/tree/57de679c287379e24cf829502325e29b75de5b69) | [Direct turn references and synthetic text-only example](https://github.com/xhluca/cursor-session-interoperability/blob/57de679c287379e24cf829502325e29b75de5b69/docs/format.md), verified upstream against Cursor CLI 2026.03.20.                                                                                                                                                                                          |

On 2026-11-09 the pinned cursor-history `parseStoreDb` and
`discoverStoreSessions` read this fixture: three visible messages, two linked
tool outcomes, and one session from both representations. After the result was
removed, the parser reported partial coverage and preserved the other messages
and result. Native Cursor resume and desktop reconstruction were outside this
check.

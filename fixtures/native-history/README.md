# Synthetic native history fixtures

These files are test inputs for `llm-history`. They are entirely synthetic and
can be used directly from a checkout. No upstream repository, native harness,
network access, credentials or private history is needed to use them. The
[harness notes](../../docs/harness-history/README.md) record the source pins and
format references behind this corpus.

| Fixture | Contents |
| --- | --- |
| [`codex/`](codex/) | An archived compressed parent with an exact decoded byte cutoff and a later parent-only record; a paginated child, copied legacy fork, and copied subagent prefix; compaction and rollback; state DB, lagging history projection cursor and append-only name index; all 13 outer record families and 16 persisted response families across the linked rollouts and wire samples. Compatibility files include bare history and enveloped records with no ordinal, summary-only and numeric-window compaction. |
| [`claude-code/`](claude-code/) | Split assistant response and parallel results with different parents; user image and nested result image; usage and opaque reasoning; progress, attachment, title, tag, summary and content replacement; compacted chain, remapped fork, and nested child transcripts with actual spawning tool-use IDs in sidecars. |
| [`opencode/`](opencode/) | SQL for a current database covering all 11 transcript message tags, inline/split/mixed content, all four tool states, an old native-field spelling, media blobs, instructions, a copied fork and a distinct delegated child. A second snapshot mutates a tool without advancing message `seq`; a third removes the source session. `v1.sql` covers both legacy message tags, all 12 V1 part tags, and separate part IDs. |
| [`pi/`](pi/) | All 11 v3 entry tags, branches and an independent copied fork, usage, context replacement and suppression, compaction, custom state and messages, shell, and ordered tool/media content. A v1 fixture exercises migration of entry IDs, compaction position, and the `hookMessage` role. |
| [`tau/`](tau/) | All 10 Python Tau entry tags across current and compatible historical shapes, including an ignored legacy leaf, two compaction boundary forms, tree replay, media and signed thinking, and conflicting project/global titles plus a separate archive index. |
| [`cursor/`](cursor/) | One CLI session with matching JSONL and SQLite representations, selected and abandoned conversation roots, two same-name tool calls with results in reverse order, and a mutation that removes one selected result blob. |
| [`recovery/unsafe.jsonl`](recovery/unsafe.jsonl) | Deliberately outside the positive native corpus: a future record with precise numbers and explicit null, duplicate keys with CRLF, invalid UTF-8, malformed JSON and an unterminated last line. Use this to test retention and cutoff policy. |
| [`change-detection/scenarios.json`](change-detection/scenarios.json) | Recipes for before/after native source snapshots used to test when an adapter can skip work. Reuses the JSONL and SQLite inputs above. |

`codex/wire/` contains independent record samples for decoding. The linked
rollouts under `codex/home/` are the scenario for capture and lineage. The
separate [`lineage.json`](codex/lineage.json) records the expected exclusive
boundary. Preserve the JSONL bytes when copying the fixtures; the prefix offset
is into the original decoded stream, including LF and multibyte text.

For database tests, apply SQL to a fresh temporary SQLite file:

- Codex: apply `state.sql` as `state_5.sqlite` and `thread-history.sql` as
  `thread_history_1.sqlite` in a copied fixture home. Rebase `/fixture/codex/`
  in the SQL `rollout_path` values onto that home when testing native path
  resolution. Tau index paths use `/fixture/tau/` in the same way. Pi's fork
  path uses `/fixture/pi/`. These roots are synthetic.
- OpenCode: apply `schema.sql` and `seed.sql` for the initial snapshot. Apply
  `settle-tool.sql` and then `delete-source-rows.sql` between acquisitions.
  Apply `v1.sql` alone to a separate database. The current and V1 schema
  subsets come from pinned upstream SQL; the rows and transitions are authored.
- Cursor: copy `cursor/home/` to a temporary root. Apply `cursor/store.sql` to
  `store.db` beside the copied `chats/*/*/meta.json`. The matching transcript is
  already under `projects/`. Apply `cursor/drop-active-result.sql` for a second
  snapshot with a missing result. The `/fixture/cursor` workspace is synthetic;
  it is not needed on disk.

The [change detection tests](test_change_detection.py) materialize the recipes
in temporary directories. For file sources, copy the declared home, split the
transcript at `initial_lines`, and append the remaining original bytes for the
next snapshot. Apply the listed SQL files to create SQLite inputs. Keep native
record bytes unchanged when splitting; only the explicit Pi replacement scenario
edits a same-length text field. Observe unchanged inputs twice to exercise a
no-op. Follow the rebasing notes above when a test resolves catalog paths.

| Harness | Change evidence to exercise |
| --- | --- |
| Codex | Child rollout appends while both databases and the name index stay unchanged. A separate catalog-only commit updates the SQLite WAL and leaves the rollout unchanged. |
| Claude Code | A child transcript grows while its parent and agent sidecar stay unchanged. |
| Pi | A file appends, or an atomic replacement changes its identity while preserving size and mtime. |
| Hugging Face Tau | A transcript grows while its indexes stay unchanged; a valid unindexed transcript is also discoverable on disk. |
| OpenCode | A retained read-only SQLite connection sees a commit when an existing tool settles at the same maximum message sequence. The next mutation deletes the session's rows. |

These tests validate the fixture transitions independently of the importer.
To run the corpus checks using the repo's existing pytest environment:

```sh
uv run --frozen --no-sync --project tools/architecture python -m pytest fixtures/native-history -q
```

The compressed-boundary check uses the local `zstd` CLI when available. Future
importer tests can read the same artifacts with the application's own decoder.

## Source validation for this snapshot

Validated on **2026-11-08** against the immutable revisions in the linked harness
notes. This is provenance for the synthetic shapes. Upstream checks are useful
when adding or revising a fixture; they are independent of our test runs.

| Harness | What ran |
| --- | --- |
| Codex | Compiled the pinned `codex-history` and `codex-protocol` crates and ran the exact `decode_rollout_line` / `parse_rollout_line_bytes` functions from `codex-rs/rollout/src/lib.rs` in a temporary program. Accepted 65 records across the modern and compatible enveloped fixtures, without decoding known response samples as `Other`. The zstd fixture was decompressed for that codec check. Replayed the state and thread-history SQL migrations before extracting fixture table definitions. The 0.20 bare fixture was checked against its historical source writer/reader; the old CLI was not run. |
| Claude Code | Ran the pinned public Python SDK `get_session_messages`, `list_subagents` and `get_subagent_messages` against the fixture home. It recovered both parallel results once, followed the compact summary, and read the nested tool/agent sidecar links. The public reader exposes raw message dictionaries; this verifies reader behavior, with the closed CLI writer outside that check. |
| OpenCode | Ran pinned `packages/core/src/database/schema.gen.ts` against empty SQLite. Inserted the fixture SQL and decoded 25 current messages and five parts with the pinned Effect schemas and stored-user/legacy adapters before mutation; decoded again after settlement. Checked deletion behavior and decoded two V1 messages and 16 parts with the pinned V1 schemas. |
| Pi | Ran the pinned `parseSessionEntries`, `migrateSessionEntries` and `buildSessionProjection` functions with their source helpers in Bun. The 21-record main fixture, six-record fork and four-record v1 fixture all loaded; checked branch selection, summaries, context suppression and the v1 migration. Pi's source reader is permissive about JSON fields, so this is a parser/projection check. |
| Tau | Ran the pinned `entries_from_json_lines`, `entry_to_json_line`, `SessionState.from_entries`, and `SessionManager` using the upstream frozen Python dependencies. Round-tripped 21 transcript entries, checked compaction and the ignored legacy leaf, and confirmed the project title wins over the older global entry while the archive is listed separately. |

The [Cursor review](../../docs/harness-history/cursor/README.md) records the
2026-11-09 check against the pinned cursor-history reader.

Coverage targets durable transcript, content, identities and lifecycle evidence.
Provider and extension payloads remain open; representative opaque values are
included. Runtime-only events and every historical release are outside this
corpus. Add a new case when an observed shape or behavior requires one.

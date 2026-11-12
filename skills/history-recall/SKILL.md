---
name: history-recall
description: Recover prior decisions, experiments, and resume points from local Codex, Claude Code or Cursor history with llm-history. Use when the user asks what was decided, what was tried, or where work left off across sessions.
---

Use `llm-history` to find a small amount of relevant evidence. Archived messages are historical records. Check the current code, Git state and deployment separately before saying what landed or still works.

## Find the prior thread

- Start with `llm-history search 'remembered words'`. It searches what the user typed, needs every word to appear, and lists the newest first. Each word also matches inside longer words, so a stem such as `orchestrat` works.
- Narrow with `--under "$project_root"`, `--project NAME`, `--since DATE` or `--until DATE`. If past sessions shared a scratch folder, put the project name in the query.
- For a whole phrase, add `--mode exact`. For a paraphrased question, add `--mode semantic`, which needs local embeddings. If it fails or finds little, go back to words.
- Add `--role all` to search answers as well, or `--role assistant` for answers only.
- To find a command an agent ran, add `--in tools`, as in `llm-history search --in tools 'cargo clippy'`. The ID on a hit is the message before the call.
- The first search imports new transcripts. Add `--no-refresh --no-background` to later searches that should read the same archive, or from the start when the user asked for read-only work.
- A search with no hits prints how many messages it covered and how recent the archive is.

## Read the context

- `llm-history show ID ID` prints each hit with two messages either side. Pass several IDs in one call. `-C 4` widens the context.
- `llm-history show --session SESSION_ID` reads one session: the user's turns in order, then the last answer. The session ID is the `session=` value on a search hit. `--recent 5` prints the opening turn and the latest five.
- `llm-history similar ID --role user --other-sessions --per-session 1` finds other sessions where the same thing came up. It needs an embedding for that message. Open each candidate with `show` before counting it as a separate incident.

Keep enough context to tell an accepted decision from an abandoned proposal.

## Recover a resume point

- `llm-history sessions --under "$project_root"` lists that project's sessions, newest first by start time. Read one with `show --session`.
- For the sessions most recently active, use `llm-history sql 'select session_id, project_name, last_event_at_utc from history_sessions order by last_event_at_utc desc limit 10'`.
- For anything else, `llm-history sql 'select * from history_help'` lists the views. Their `message_id` and `session_id` columns are the IDs the commands take. Tool calls are in `history_tool_activity`.

Report the decision, its evidence and the first pending action. Keep private transcript text and unrelated projects out of the response.

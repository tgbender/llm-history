# Parity with Python llm-history

Reference: `a7aa830`. Last checked 2026-10-28 by importing the same live history into both and comparing.

## Matches

- Messages: text, role, phase, exclusion, timestamp and turn ID for every shared message.
- `sessions`: provider, role, title, model, effort, cwd and start time.
- `usage`: per-period totals and Python's field names.
- `search`: result order across the lexical filters (role, source, date, project, cwd, under, subagents), answers chosen for matched questions, and passages.
- `show` context windows.
- `history_*` SQL views: same names and columns.
- Python's option names, including the explicit defaults (`--include-subagents`, `--refresh`, `--background`, `sql --redact`, `--top`, `--max-rows`).

## Deliberate differences

- Profiles: independent stores selected with `--profile`. Usage and status are split by profile.
- Search, context and session listings expose short integer IDs and compact JSON. Native session and item IDs remain available in SQL.
- Timestamps in text and JSON output are ISO UTC (`...Z`).
- The CLI's default search differs from Python: it covers user messages (`--role all` restores both roles) and uses `--mode words`, where every whitespace-separated word must appear as a literal, case-insensitive string. `--mode lexical --role all` gives the Python behavior. `words` shares the rules below for `exact`.
- `--mode exact` is Rust-only. It matches the query as a literal, case-insensitive string, punctuation included, in the same text search covers. Hits are newest first, keep the matched message instead of swapping in its answer, and don't use the FTS index. `--rerank` is refused because hits are ordered by date. When a cap omits matches, JSON includes `matched` alongside the result array.
- `similar MESSAGE_ID` is Rust-only. It ranks other messages against the selected message's stored Gemma 2 embeddings.
- Text tables print each row on one line. Piped output keeps every cell whole, since agents read it. Only on a terminal are rows fitted to `COLUMNS` (default 120): short identifier-like cells stay whole and longer text is shortened with an ellipsis (paths keep their end). Python truncates every cell at 80 characters, and `sessions` did here until this was removed; redaction still applies.
- Codex approval reviews (`source.subagent.other = "guardian"`) are session kind `guardian`, titled "Guardian review", and every verdict is agent protocol. They stay hidden from default listings, search and embedding, like subagents. Python classifies them as plain subagents and only flags verdicts that start with `risk_level`. `--kind` is Rust-only. A guardian hit keeps its prompt and adds a `verdict` field, because a bare verdict does not say what was reviewed. Most of a guardian prompt is a quoted transcript, so search and snippets cover only the `>>> APPROVAL REQUEST` block that holds the reviewed action. `show` keeps that block when the prompt exceeds its budget, and `--full` returns the whole prompt. Prompts without a block, such as reviews of native tool calls, are searched whole. Codex sessions without a parent have an empty relation instead of Python's `continued_from`.
- Session titles are one line of at most 120 characters. Codex threads use the generated `name` first, then a title or preview that is not just the first message, then a subagent's task path, then the first message. Python reads only `title` and `preview`, which Codex fills with the whole first message until it names the thread; Claude fallbacks keep Python's order. Full messages stay in the events and `native_threads`.
- The Rust store keeps one fact table with views instead of Python's separate canonical tables.

## Open

- A native Windows x64 release build passed synthetic import, lexical search, raw verification, and `db compact`. Full Python parity and local inference on Windows still need a separate run. Linux has no current parity run.

"""Generate synthetic reference facts from the frozen sibling Python checkout."""
import json
from pathlib import Path
import runpy
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = ROOT.parent / "llm-history"
COMMIT = "a7aa83019b73469e189c27d05703bed41f856e4d"


def main():
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REFERENCE, text=True).strip()
    assert actual == COMMIT, "reference checkout changed; review the oracle before regenerating"
    from llm_history.source_adapters.codex.parser import parse_session_snapshot
    from llm_history.source_adapters.claude.parser import parse_claude_snapshot
    from llm_history.storage.normalized_columns import INSERT_COLUMNS
    helpers = runpy.run_path(str(REFERENCE / "tests/test_claude.py"))
    search = runpy.run_path(str(REFERENCE / "tests/test_search.py"))
    from llm_history.lexical import code_search_aliases
    cases = ["ordinary words", "parseHTTPServer foo_bar FTS2 v2.0.0", "C++ C# F# .NET", r"src/history.rs C:\Users\work\main.rs", "Straße_Name Σίσυφος_Data ﬃValue", "HTTPServer2 duckdb::Connection /tmp/.codex-work"]
    golden = {"reference_commit": COMMIT, "sources": [], "aliases": [{"text": text, "expected": code_search_aliases(text)} for text in cases]}
    with tempfile.TemporaryDirectory() as directory:
        temporary = Path(directory)
        codex = search["_write_history"](temporary / "codex")
        claude = temporary / "claude/projects/project/session-claude.jsonl"
        helpers["write_jsonl"](claude, helpers["claude_records"]())
        for harness, source, parser, kind in (("codex", codex, parse_session_snapshot, "sessions"), ("claude", claude, parse_claude_snapshot, "projects")):
            output = ROOT / "fixtures" / (harness + ".jsonl")
            output.write_bytes(source.read_bytes())
            parsed = parser(source, kind)
            rows = [dict(zip(INSERT_COLUMNS["messages"], r)) for r in parsed.rows["messages"]]
            messages = [{k: r[k] for k in ("line_no", "role", "phase", "text")} for r in rows]
            tokens = [dict(zip(INSERT_COLUMNS["token_events"], r)) for r in parsed.rows["token_events"]]
            golden["sources"].append({"harness": harness, "file": output.name,
                "native_session_id": parsed.canonical_session.native_session_id,
                "messages": messages, "tools": len(parsed.rows["tool_calls"]),
                "tool_results": len(parsed.rows["tool_outputs"]),
                "input_tokens": sum(r.get("last_input_tokens") or 0 for r in tokens),
                "output_tokens": sum(r.get("last_output_tokens") or 0 for r in tokens)})
    (ROOT / "fixtures/reference.json").write_text(json.dumps(golden, indent=2) + "\n")
    print(json.dumps(golden, indent=2))


if __name__ == "__main__":
    main()

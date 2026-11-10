"""Check the pinned Codex documentation inventories against a source checkout.

Uses the architecture environment's Rust parser and stdlib SQLite. No Codex build,
live database, network request or source mutation is involved.
"""
from __future__ import annotations

import argparse
from functools import cache
import json
from pathlib import Path
import re
import sqlite3
import subprocess
import time

from tree_sitter import Language, Parser
import tree_sitter_rust

ROOT = Path(__file__).resolve().parents[2]
DOCS = ROOT / "docs/harness-history/codex"
REVISION = "2bc8e7f5459b084afa482c7aac3f56fef36286b3"
PUBLIC_ROOT = f"https://github.com/openai/codex/blob/{REVISION}/"
ENUMS = {
    "RolloutItem": "codex-rs/history/src/lib.rs",
    "ResponseItem": "codex-rs/protocol/src/models.rs",
    "EventMsg": "codex-rs/protocol/src/protocol.rs",
    "TurnItem": "codex-rs/protocol/src/items.rs",
    "ThreadItemPayload": "codex-rs/app-server-protocol/src/protocol/v2/item.rs",
}
HISTORICAL_SNAPSHOTS = [
    ("0.20.0", "45aed5edd139c38132de044596fdd55e47236657", "2025-08-08", "unwrapped", [
        "codex-rs/core/src/protocol.rs", "codex-rs/core/src/models.rs", "codex-rs/core/src/rollout.rs"]),
    ("0.50.0", "b4123b7b1db22a3c0a8b133a23c7b30a477d7b65", "2025-10-25", "enveloped", [
        "codex-rs/protocol/src/protocol.rs", "codex-rs/protocol/src/models.rs", "codex-rs/core/src/rollout/recorder.rs", "codex-rs/core/src/rollout/policy.rs"]),
    ("0.100.0", "8272f9a71ee7364d8de992418103628f8a17eb6f", "2026-02-12", "enveloped", [
        "codex-rs/protocol/src/protocol.rs", "codex-rs/protocol/src/models.rs", "codex-rs/core/src/rollout/recorder.rs", "codex-rs/core/src/rollout/policy.rs"]),
    ("0.140.0", "6506579001c322927a3e4bd440563267a7ac6c1f", "2026-06-15", "enveloped", [
        "codex-rs/protocol/src/protocol.rs", "codex-rs/protocol/src/models.rs", "codex-rs/rollout/src/recorder.rs", "codex-rs/rollout/src/policy.rs"]),
]
MIGRATIONS = {
    "state": ("migrations", ["state_5.sqlite"]),
    "thread_history": ("thread_history_migrations", ["thread_history_1.sqlite"]),
    "logs": ("logs_migrations", ["logs_2.sqlite"]),
    "goals": ("goals_migrations", ["goals_1.sqlite"]),
    "memories": ("memory_migrations", ["memories_1.sqlite", "memories_v2_1.sqlite"]),
    "queue": ("queue_migrations", ["queue_1.sqlite"]),
}


def enum_variants(source: Path, relative: str, name: str) -> set[str]:
    parser = Parser(Language(tree_sitter_rust.language()))
    tree = parser.parse((source / relative).read_bytes())
    pending = [tree.root_node]
    while pending:
        node = pending.pop()
        if node.type == "enum_item" and node.child_by_field_name("name").text.decode() == name:
            return {
                child.child_by_field_name("name").text.decode()
                for child in node.child_by_field_name("body").named_children
                if child.type == "enum_variant"
            }
        pending.extend(node.named_children)
    raise AssertionError(f"Missing enum {name} in {relative}")


def identifier(value: str) -> str:
    return '"' + value.replace('"', '""') + '"'


@cache
def source_text(source: Path, revision: str, relative: str) -> str:
    assert revision in {REVISION, *(s[1] for s in HISTORICAL_SNAPSHOTS)}, revision
    assert relative.startswith("codex-rs/") and ".." not in Path(relative).parts, relative
    if revision == REVISION:
        return (source / relative).read_text()
    return subprocess.check_output(["git", "show", f"{revision}:{relative}"], cwd=source, timeout=10, text=True)


def attributes(node) -> list[str]:
    result = []
    previous = node.prev_named_sibling
    while previous is not None and previous.type in {"attribute_item", "line_comment", "block_comment"}:
        if previous.type == "attribute_item":
            result.append(previous.text.decode())
        previous = previous.prev_named_sibling
    return list(reversed(result))


def historical_catalog(source: Path) -> dict:
    selected = {
        "RolloutLine", "SessionMeta", "SessionMetaLine", "SessionMetaWithGit",
        "CompactedItem", "TurnContextItem", "ContentItem", "FunctionCallOutputPayload",
        "FunctionCallOutputContentItem", "SessionSource", "SubAgentSource", "SessionStateSnapshot",
        "SavedSession", "LocalShellAction", "LocalShellExecAction", "LocalShellStatus", "WebSearchAction",
        "MessagePhase", "ReasoningItemContent", "ReasoningItemReasoningSummary",
    }
    parser = Parser(Language(tree_sitter_rust.language()))
    snapshots = []
    for version, revision, date, family, files in HISTORICAL_SNAPSHOTS:
        enums, declarations, codecs = {}, [], []
        for relative in files:
            raw = source_text(source, revision, relative).encode()
            pending = [parser.parse(raw).root_node]
            while pending:
                node = pending.pop()
                if node.type in {"enum_item", "struct_item"}:
                    name = node.child_by_field_name("name").text.decode()
                    url = f"https://github.com/openai/codex/blob/{revision}/{relative}#L{node.start_point.row+1}-L{node.end_point.row+1}"
                    if name in {"RolloutItem", "ResponseItem", "EventMsg"}:
                        variants = [{"rust_variant": v.child_by_field_name("name").text.decode(),
                                     "attributes": attributes(v), "rust_declaration": v.text.decode(),
                                     "source": f"https://github.com/openai/codex/blob/{revision}/{relative}#L{v.start_point.row+1}-L{v.end_point.row+1}"}
                                    for v in node.child_by_field_name("body").named_children if v.type == "enum_variant"]
                        enums[name] = {"attributes": attributes(node), "source": url, "variants": variants}
                    elif name in selected:
                        declarations.append({"rust_type": name, "attributes": attributes(node), "rust_declaration": node.text.decode(), "source": url})
                elif node.type == "impl_item" and "FunctionCallOutputPayload" in node.text.decode().split("{", 1)[0] and re.search(r"Serialize|Deserialize", node.text.decode().split("{", 1)[0]):
                    codecs.append({"kind": "custom_output_codec", "rust_declaration": node.text.decode(), "source": f"https://github.com/openai/codex/blob/{revision}/{relative}#L{node.start_point.row+1}-L{node.end_point.row+1}"})
                elif node.type == "function_item" and node.child_by_field_name("name").text.decode() in {"write_line", "write_rollout_item", "record_items", "resume", "load_rollout_items", "should_persist_response_item", "is_persisted_response_item", "should_persist_event_msg"}:
                    codecs.append({"kind": "writer_reader_or_policy", "rust_name": node.child_by_field_name("name").text.decode(), "rust_declaration": node.text.decode(), "source": f"https://github.com/openai/codex/blob/{revision}/{relative}#L{node.start_point.row+1}-L{node.end_point.row+1}"})
                pending.extend(node.named_children)
        snapshots.append({"release": version, "source_revision": revision, "release_commit_date": date,
                          "wire_family": family, "release_range_claim": None,
                          "inspected_sources": [f"https://github.com/openai/codex/blob/{revision}/{p}" for p in files],
                          "enums": enums, "payload_declarations": sorted(declarations, key=lambda d: (d['rust_type'], d['source'])),
                          "codec_and_reader_evidence": sorted(codecs, key=lambda c: c['source'])})
    return {"source_revision": REVISION, "reviewed_at": "2026-11-07",
            "scope": "Four explicitly inspected release snapshots; evidence of saved formats, not an exhaustive release timeline or historical acceptance allowlist.",
            "snapshots": snapshots}


def storage_schema(source: Path) -> dict:
    result = {
        "source_revision": REVISION,
        "reviewed_at": "2026-11-07",
        "method": "Replay public SQL migrations in empty in-memory SQLite databases; inspect final DDL. Excludes SQLx's runtime migration bookkeeping table and runtime-created state.",
        "databases": [],
    }
    for role, (folder, filenames) in MIGRATIONS.items():
        paths = sorted((source / "codex-rs/state" / folder).glob("*.sql"))
        assert paths, folder
        con = sqlite3.connect(":memory:")
        deadline = time.monotonic() + 10
        con.set_progress_handler(lambda: time.monotonic() >= deadline, 10_000)
        try:
            for path in paths:
                con.executescript(path.read_text())
            tables = []
            for name, sql in con.execute("SELECT name, sql FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").fetchall():
                columns = [
                    {"name": row[1], "type": row[2], "not_null": bool(row[3]),
                     "default": row[4], "primary_key_position": row[5], "hidden": row[6]}
                    for row in con.execute(f"PRAGMA table_xinfo({identifier(name)})")
                ]
                indexes = []
                for row in con.execute(f"PRAGMA index_list({identifier(name)})").fetchall():
                    index_sql = con.execute("SELECT sql FROM sqlite_master WHERE type='index' AND name=?", (row[1],)).fetchone()
                    indexes.append({"name": row[1], "unique": bool(row[2]), "origin": row[3], "partial": bool(row[4]), "sql": index_sql[0] if index_sql else None})
                foreign_keys = [
                    {"id": r[0], "position": r[1], "target_table": r[2], "from": r[3], "to": r[4], "on_update": r[5], "on_delete": r[6], "match": r[7]}
                    for r in con.execute(f"PRAGMA foreign_key_list({identifier(name)})")
                ]
                tables.append({"name": name, "sql": sql, "columns": columns,
                               "indexes": sorted(indexes, key=lambda i: i["name"]), "foreign_keys": foreign_keys})
            triggers = [{"name": r[0], "table": r[1], "sql": r[2]} for r in con.execute("SELECT name, tbl_name, sql FROM sqlite_master WHERE type='trigger' ORDER BY name")]
            result["databases"].append({
                "role": role, "filenames": filenames,
                "migration_sources": [PUBLIC_ROOT + p.relative_to(source).as_posix() for p in paths],
                "tables": tables, "triggers": triggers,
            })
        finally:
            con.close()
    return result


def check_catalog(path: Path, key: str, expected: set[str]) -> None:
    data = json.loads(path.read_text())
    assert data["source_revision"] == REVISION, path.name
    entries = data[key]
    actual = [entry["rust_variant"] for entry in entries]
    assert len(actual) == len(set(actual)), f"Duplicate variants in {path.name}:{key}"
    assert set(actual) == expected, f"{path.name}:{key}: missing={sorted(expected-set(actual))}, extra={sorted(set(actual)-expected)}"
    print(f"{path.name}:{key}: {len(actual)} variants covered")


def check_source_url(source: Path, target: str) -> None:
    match = re.fullmatch(r"https://github.com/openai/codex/blob/([0-9a-f]+)/([^#]+)(?:#L(\d+)(?:-L(\d+))?)?", target)
    assert match, f"Unpinned source reference: {target}"
    text = source_text(source, match[1], match[2])
    if match[3]:
        assert 0 < int(match[3]) <= int(match[4] or match[3]) <= len(text.splitlines()), target


def check_declarations(source: Path) -> int:
    count = 0
    def walk(value):
        nonlocal count
        if isinstance(value, dict):
            declaration, url = value.get("rust_declaration"), value.get("source")
            if isinstance(declaration, str) and isinstance(url, str):
                match = re.fullmatch(r"https://github.com/openai/codex/blob/([0-9a-f]+)/([^#]+)(?:#.*)?", url)
                assert match, url
                assert declaration in source_text(source, match[1], match[2]), f"Declaration differs from source: {url}"
                count += 1
            for child in value.values():
                walk(child)
        elif isinstance(value, list):
            for child in value:
                walk(child)
    for path in DOCS.glob("*.json"):
        data = json.loads(path.read_text())
        assert data["source_revision"] == REVISION, path.name
        assert data["reviewed_at"] == "2026-11-07", path.name
        walk(data)
    return count


def check_links(source: Path) -> None:
    links = set()
    for path in DOCS.glob("*.md"):
        text = path.read_text()
        definitions = dict(re.findall(r"^\[([^]]+)\]:\s*(\S+)\s*$", text, re.M))
        references = re.findall(r"\[[^]\n]+\]\[([^]]+)\]", text)
        assert set(references) <= definitions.keys(), f"Undefined references in {path.name}: {set(references)-definitions.keys()}"
        targets = list(definitions.values()) + re.findall(r"\]\(([^\s)]+)(?:\s+[^)]*)?\)", text)
        for target in targets:
            if target.startswith("https://github.com/openai/codex/blob/"):
                links.add(target)
            elif not re.match(r"[a-zA-Z]+:", target) and not target.startswith("#"):
                assert not target.startswith("/"), f"Local absolute path in {path.name}: {target}"
                assert (path.parent / target.split("#")[0]).exists(), f"Broken local link in {path.name}: {target}"
        assert not re.search(r"/(?:Users|home)/|[A-Z]:\\", text), f"Machine-specific path in {path.name}"
    def collect(value):
        if isinstance(value, str) and value.startswith("https://github.com/openai/codex/blob/"):
            links.add(value)
        elif isinstance(value, dict):
            for child in value.values():
                collect(child)
        elif isinstance(value, list):
            for child in value:
                collect(child)
    for path in DOCS.glob("*.json"):
        collect(json.loads(path.read_text()))
    for target in sorted(links):
        check_source_url(source, target)
    print(f"Documentation links: {len(links)} unique pinned source references checked")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path, help="Read-only Codex checkout at the documented revision")
    parser.add_argument("--write-storage-schema", action="store_true", help="Regenerate only storage-schema.json from the pinned migration files")
    parser.add_argument("--write-historical-catalog", action="store_true", help="Regenerate only historical-snapshots.json from four explicitly pinned release snapshots")
    args = parser.parse_args()
    source = args.source.resolve()
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, timeout=10, text=True).strip()
    assert revision == REVISION, f"Expected {REVISION}, got {revision}"
    if args.write_historical_catalog:
        (DOCS / "historical-snapshots.json").write_text(json.dumps(historical_catalog(source), indent=2) + "\n")
        print("Wrote historical-snapshots.json: four explicitly inspected release snapshots")
        return
    schema = storage_schema(source)
    schema_path = DOCS / "storage-schema.json"
    if args.write_storage_schema:
        schema_path.write_text(json.dumps(schema, indent=2) + "\n")
        print(f"Wrote {schema_path.name}: {sum(len(d['tables']) for d in schema['databases'])} final tables across {len(schema['databases'])} migration families")
        return
    assert json.loads(schema_path.read_text()) == schema, "storage-schema.json differs from fresh migration DDL"
    assert json.loads((DOCS / "historical-snapshots.json").read_text()) == historical_catalog(source), "Historical declaration catalog differs from pinned snapshots"
    expected = {name: enum_variants(source, relative, name) for name, relative in ENUMS.items()}
    for filename, key, enum in [
        ("record-catalog.json", "rollout_items", "RolloutItem"),
        ("record-catalog.json", "response_items", "ResponseItem"),
        ("persistence-matrix.json", "event_messages", "EventMsg"),
        ("persistence-matrix.json", "rollout_items", "RolloutItem"),
        ("lifecycle-catalog.json", "turn_items", "TurnItem"),
        ("lifecycle-catalog.json", "thread_item_payloads", "ThreadItemPayload"),
    ]:
        check_catalog(DOCS / filename, key, expected[enum])
    for path in DOCS.glob("*.json"):
        json.loads(path.read_text())
    print(f"Exact declaration excerpts: {check_declarations(source)} source matches checked")
    examples = 0
    for path in (DOCS / "examples").glob("*.jsonl"):
        for n, line in enumerate(path.read_text().splitlines(), 1):
            if line.strip():
                assert isinstance(json.loads(line), dict), f"{path.name}:{n} must be an object"
                examples += 1
    check_links(source)
    print(f"JSON examples: {examples} object records checked; storage DDL matches all six migration families")


if __name__ == "__main__":
    main()

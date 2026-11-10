"""Check fixture topology and state transitions independently of our importer."""

import hashlib
import json
import shutil
import sqlite3
import subprocess
from decimal import Decimal
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent
BODY = "Fixture greeting: café 🦀\n"


def records(path):
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").split("\n")
        if line
    ]


def apply_sql(db, *paths):
    for path in paths:
        db.executescript(path.read_text(encoding="utf-8"))
    assert db.execute("PRAGMA foreign_key_check").fetchall() == []


def test_codex_prefix_is_in_original_decoded_bytes(tmp_path):
    zstd = shutil.which("zstd")
    if zstd is None:
        pytest.skip("zstd CLI is needed to inspect the compressed fixture")
    root = ROOT / "codex"
    expected = json.loads((root / "lineage.json").read_text())
    parent = root / expected["parent"]
    decoded = subprocess.run(
        [zstd, "-q", "-d", "-c", str(parent)],
        capture_output=True,
        check=True,
        timeout=5,
    ).stdout
    child = records(root / expected["child"])
    base = child[0]["payload"]["history_base"]
    cut = base["end_byte_offset"]
    assert cut == expected["end_decoded_byte"]
    assert decoded[cut - 1 : cut] == b"\n"
    prefix = [json.loads(line) for line in decoded[:cut].split(b"\n") if line]
    assert base["thread_id"] == prefix[0]["payload"]["id"]
    assert [row["ordinal"] for row in prefix] == list(
        range(base["end_ordinal_exclusive"])
    )
    assert child[0]["ordinal"] == base["end_ordinal_exclusive"]
    suffix = json.loads(decoded[cut:])
    assert suffix["payload"]["content"][0]["text"] == expected["outside_prefix"]
    assert (
        prefix[2]["payload"]["content"][0]["text"]
        == child[2]["payload"]["content"][0]["text"]
        == BODY
    )
    assert prefix[2]["timestamp"] != child[2]["timestamp"]

    with sqlite3.connect(tmp_path / "thread_history_1.sqlite") as db:
        apply_sql(db, root / "thread-history.sql")
        offset, ordinal = db.execute(
            "select next_rollout_byte_offset,next_rollout_ordinal from thread_history_projection_state where thread_id=?",
            (base["thread_id"],),
        ).fetchone()
    assert ordinal < base["end_ordinal_exclusive"]
    assert offset == sum(len(line) + 1 for line in decoded.split(b"\n")[:ordinal])


def test_codex_catalog_and_wire_families(tmp_path):
    root = ROOT / "codex"
    files = list((root / "home/sessions").rglob("*.jsonl"))
    rows = [row for p in files for row in records(p)] + records(
        root / "wire/record-families.jsonl"
    )
    assert {r["type"] for r in rows} == {
        "session_meta",
        "dynamic_tools_updated",
        "response_item",
        "inter_agent_communication",
        "inter_agent_communication_metadata",
        "compacted",
        "turn_context",
        "token_usage_record",
        "world_state",
        "retained_context",
        "security_risk_score",
        "event_msg",
        "realtime_item",
    }
    assert {r["payload"]["type"] for r in rows if r["type"] == "response_item"} == {
        "additional_tools",
        "message",
        "agent_message",
        "reasoning",
        "local_shell_call",
        "function_call",
        "tool_search_call",
        "function_call_output",
        "tool_search_output",
        "custom_tool_call",
        "custom_tool_call_output",
        "web_search_call",
        "image_generation_call",
        "configuration_update",
        "compaction_summary",
        "context_compaction",
    }
    with sqlite3.connect(tmp_path / "state_5.sqlite") as db:
        apply_sql(db, root / "state.sql")
        catalog = db.execute(
            "select id,rollout_path,archived,history_mode from threads order by id"
        ).fetchall()
    assert len(catalog) == 4
    for thread, location, archived, mode in catalog:
        source = root / "home" / location.removeprefix("/fixture/codex/")
        assert source.is_file() and thread in source.name
        assert archived == int(source.parent.name == "archived_sessions")
        if not archived:
            header, *items = records(source)
            assert (
                header["payload"]["id"] == thread
                and header["payload"]["history_mode"] == mode
            )
            if mode == "legacy":
                assert all("ordinal" not in row for row in [header, *items])
            if start := header["payload"].get("subagent_history_start_ordinal"):
                assert [row["ordinal"] for row in items if row["ordinal"] < start] == [
                    1,
                    2,
                ]
    names = records(root / "home/session_index.jsonl")
    assert (
        names[-1]["id"] == names[-2]["id"]
        and names[-1]["thread_name"] != names[-2]["thread_name"]
    )


def test_claude_tool_links_and_fork():
    project = ROOT / "claude-code/home/projects/-synthetic-project"
    parent, fork, compacted = sorted(project.glob("*.jsonl"))
    source = records(parent)
    fork_rows = records(fork)
    assert fork_rows[0]["forkedFrom"] == {
        "sessionId": parent.stem,
        "messageUuid": source[0]["uuid"],
    }
    assert (
        fork_rows[0]["uuid"] != source[0]["uuid"]
        and fork_rows[0]["message"] == source[0]["message"]
    )
    comp = records(compacted)
    boundary = next(r for r in comp if r.get("subtype") == "compact_boundary")
    assert boundary["logicalParentUuid"] in {r.get("uuid") for r in comp}
    assert (
        next(r for r in comp if r.get("isCompactSummary"))["parentUuid"]
        == boundary["uuid"]
    )

    agents = {
        p.stem.removeprefix("agent-"): p
        for p in (project / parent.stem / "subagents").rglob("*.jsonl")
    }
    assert set(agents) == {"a123456", "a654321"}
    for agent, path in agents.items():
        meta = json.loads(path.with_suffix(".meta.json").read_text())
        spawning = (
            records(agents[meta["parentAgentId"]])
            if "parentAgentId" in meta
            else source
        )
        assert any(
            part.get("id") == meta["toolUseId"]
            and part["type"] == "tool_use"
            and part["name"] == "Agent"
            for row in spawning
            if isinstance(content := row.get("message", {}).get("content"), list)
            for part in content
        ), agent


def test_opencode_snapshots_keep_seq_and_change_existing_rows(tmp_path):
    root = ROOT / "opencode"
    with sqlite3.connect(tmp_path / "opencode.db") as db:
        apply_sql(db, root / "schema.sql", root / "seed.sql")
        types = {
            kind for (kind,) in db.execute("select distinct type from session_message")
        }
        assert types == {
            "agent-switched",
            "model-switched",
            "location-switched",
            "user",
            "synthetic",
            "system",
            "skill",
            "shell",
            "assistant",
            "compaction",
            "idle",
        }
        latest = "select max(seq) from session_message where session_id='ses_fixture'"
        seq = db.execute(latest).fetchone()
        query = "select data,time_updated from session_part where message_id='msg_split' and ordinal=1"
        old, old_time = db.execute(query).fetchone()
        completion = "select json_extract(data,'$.time.completed') from session_message where id='msg_split'"
        assert db.execute(completion).fetchone() == (None,)
        apply_sql(db, root / "settle-tool.sql")
        new, new_time = db.execute(query).fetchone()
        assert db.execute(latest).fetchone() == seq
        assert new_time > old_time
        assert db.execute(completion).fetchone() == (new_time,)
        old, new = json.loads(old), json.loads(new)
        assert old["id"] == new["id"] and old["state"]["input"] == new["state"]["input"]
        assert (old["state"]["status"], new["state"]["status"]) == (
            "running",
            "completed",
        )
        media = new["state"]["content"][1]["uri"]
        digest, data = db.execute("select hash,data from blob").fetchone()
        assert media == "media:" + digest and hashlib.sha256(data).hexdigest() == digest
        assert db.execute(
            "select parent_id,fork_session_id from session_v2 where id='ses_fixture_fork'"
        ).fetchone() == (None, "ses_fixture")
        assert db.execute(
            "select parent_id,fork_session_id from session_v2 where id='ses_fixture_worker'"
        ).fetchone() == ("ses_fixture", None)
        assert (
            db.execute(
                "select seq,data from session_message where id='msg_user'"
            ).fetchone()
            == db.execute(
                "select seq,data from session_message where id='msg_fixture_fork_6'"
            ).fetchone()
        )
        apply_sql(db, root / "delete-source-rows.sql")
        assert db.execute(
            "select count(*) from session_message where session_id='ses_fixture'"
        ).fetchone() == (0,)
        assert db.execute(
            "select count(*) from session_message where session_id='ses_fixture_fork'"
        ).fetchone() == (11,)
        assert db.execute("select message_id from blob_ref").fetchall() == [
            ("msg_fixture_fork_6",)
        ]


def test_opencode_legacy_has_separate_part_ids(tmp_path):
    with sqlite3.connect(tmp_path / "legacy.db") as db:
        apply_sql(db, ROOT / "opencode/v1.sql")
        assert db.execute(
            "select id,message_id from part where json_extract(data,'$.type')='text' order by id"
        ).fetchall() == [
            ("prt_fixture_01", "msg_fixture_v1_user"),
            ("prt_fixture_02", "msg_fixture_v1_assistant"),
        ]
        parts = [json.loads(data) for (data,) in db.execute("select data from part")]
        assert {p["type"] for p in parts} == {
            "text",
            "subtask",
            "reasoning",
            "file",
            "tool",
            "step-start",
            "step-finish",
            "snapshot",
            "patch",
            "agent",
            "retry",
            "compaction",
        }
        assert {p["state"]["status"] for p in parts if p["type"] == "tool"} == {
            "pending",
            "running",
            "completed",
            "error",
        }


def check_parent_tree(rows, field):
    seen = set()
    for row in rows:
        assert row["id"] not in seen
        assert row.get(field) is None or row[field] in seen
        seen.add(row["id"])


def test_pi_v3_coverage_and_fork_copy():
    main, fork = (records(p) for p in sorted((ROOT / "pi/sessions").rglob("*.jsonl")))
    assert {r["type"] for r in main[1:]} == {
        "message",
        "thinking_level_change",
        "model_change",
        "usage",
        "compaction",
        "branch_summary",
        "custom",
        "custom_message",
        "context_edit",
        "label",
        "session_info",
    }
    for session in (main, fork):
        assert session[0]["type"] == "session" and session[0]["version"] == 3
        check_parent_tree(session[1:], "parentId")
    assert main[0]["id"] != fork[0]["id"] and main[1:5] == fork[1:5]
    assert any(r["type"] == "context_edit" and r["replacement"] is None for r in main)


def test_tau_coverage_and_index_roles():
    root = ROOT / "tau/sessions"
    active = next(root.rglob("fixture-tau-active.jsonl"))
    archived = root / "fixture-tau-archived.jsonl"
    current, older = records(active), records(archived)
    for rows in (current, older):
        check_parent_tree(rows, "parent_id")
    assert {r["type"] for r in current + older} == {
        "message",
        "custom_message",
        "model_change",
        "thinking_level_change",
        "compaction",
        "branch_summary",
        "label",
        "leaf",
        "session_info",
        "custom",
    }
    project, global_, archive = (
        records(p)[0]
        for p in (
            active.parent / "index.jsonl",
            root / "index.jsonl",
            root / "archive.jsonl",
        )
    )
    assert project["id"] == global_["id"] == active.stem
    assert (
        project["updated_at"] > global_["updated_at"]
        and project["title"] != global_["title"]
    )
    assert (
        archive["id"] == archived.stem
        and (ROOT / archive["path"].removeprefix("/fixture/")).is_file()
    )


def test_recovery_has_precision_duplicates_bad_bytes_and_unfinished_tail():
    raw = (ROOT / "recovery/unsafe.jsonl").read_bytes().split(b"\n")
    future, duplicate, invalid_utf8, malformed, incomplete = raw
    payload = json.loads(future, parse_float=Decimal)["payload"]
    assert payload["precise"] == Decimal("1.000000000000000000001")
    assert payload["present"] is None and "absent" not in payload
    assert duplicate.endswith(b"\r") and duplicate.count(b'"body"') == 2
    with pytest.raises(UnicodeDecodeError):
        json.loads(invalid_utf8)
    for line in (malformed, incomplete):
        with pytest.raises(json.JSONDecodeError):
            json.loads(line)

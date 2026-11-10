"""Materialize cheap-change-detection inputs without opening any real history."""

from contextlib import closing
import json
import os
from pathlib import Path
import shutil
import sqlite3

import pytest

ROOT = Path(__file__).resolve().parent
SCENARIOS = json.loads((ROOT / "change-detection/scenarios.json").read_text())


def apply_sql(connection, *inputs):
    for input_path in inputs:
        connection.executescript((ROOT / input_path).read_text(encoding="utf-8"))


def file_stamp(path):
    st = path.stat()
    return st.st_size, st.st_mtime_ns, st.st_dev, st.st_ino


def stage_file_source(destination, case):
    """Create the initial native home and return the unmodified JSONL suffix."""
    shutil.copytree(ROOT / case["home"], destination)
    transcript = destination / case["transcript"]
    lines = transcript.read_bytes().splitlines(keepends=True)
    cut = case["initial_lines"]
    assert 0 < cut < len(lines) and lines[cut - 1].endswith(b"\n")
    transcript.write_bytes(b"".join(lines[:cut]))
    for path, scripts in case.get("sqlite", {}).items():
        with closing(sqlite3.connect(destination / path)) as db:
            apply_sql(db, *scripts)
    return transcript, b"".join(lines[cut:])


@pytest.mark.parametrize("harness", SCENARIOS["files"])
def test_native_suffix_changes_only_its_transcript(tmp_path, harness):
    case = SCENARIOS["files"][harness]
    home = tmp_path / "home"
    transcript, suffix = stage_file_source(home, case)
    companions = [home / p for p in case["metadata"]]
    initial = file_stamp(transcript)
    metadata = [file_stamp(p) for p in companions]

    if harness == "codex":
        with closing(sqlite3.connect(home / "thread_history_1.sqlite")) as projection:
            assert projection.execute(
                """SELECT next_rollout_byte_offset,next_rollout_ordinal
                   FROM thread_history_projection_state WHERE thread_id=?""",
                (case["thread_id"],),
            ).fetchone() == (initial[0], json.loads(suffix.splitlines()[0])["ordinal"])

    with transcript.open("ab") as file:
        file.write(suffix)

    assert file_stamp(transcript)[0] == initial[0] + len(suffix)
    assert file_stamp(transcript)[2:] == initial[2:]
    assert [file_stamp(p) for p in companions] == metadata
    assert transcript.read_bytes() == (ROOT / case["home"] / case["transcript"]).read_bytes()


def test_codex_catalog_commit_changes_wal_without_changing_the_rollout(tmp_path):
    case = SCENARIOS["files"]["codex"]
    home = tmp_path / "home"
    transcript, _ = stage_file_source(home, case)
    catalog = home / "state_5.sqlite"
    before = file_stamp(transcript)
    with closing(sqlite3.connect(catalog)) as writer:
        assert writer.execute("PRAGMA journal_mode=WAL").fetchone() == ("wal",)
        writer.execute("PRAGMA wal_autocheckpoint=0")
        main = file_stamp(catalog)
        apply_sql(writer, case["catalog_update"])
        assert file_stamp(catalog) == main
        assert catalog.with_name(catalog.name + "-wal").stat().st_size > 32
        with closing(sqlite3.connect(catalog.as_uri() + "?mode=ro", uri=True)) as reader:
            assert reader.execute(
                "SELECT title FROM threads WHERE id=?", (case["thread_id"],)
            ).fetchone() == ("Fixture metadata-only rename",)
    assert file_stamp(transcript) == before


def test_pi_replacement_can_preserve_size_and_mtime(tmp_path):
    case = SCENARIOS["files"]["pi"]
    transcript, _ = stage_file_source(tmp_path / "home", case)
    original = transcript.read_bytes()
    delta = case["replacement"]
    changed = original.replace(delta["from"].encode(), delta["to"].encode(), 1)
    assert changed != original and len(changed) == len(original)
    for line in changed.splitlines():
        json.loads(line)
    before = file_stamp(transcript)
    replacement = transcript.with_suffix(".replacement")
    replacement.write_bytes(changed)
    os.utime(replacement, ns=(before[1], before[1]))
    replacement.replace(transcript)
    after = file_stamp(transcript)
    assert after[:2] == before[:2]
    assert after[2:] != before[2:]


def test_tau_inventory_can_include_a_file_absent_from_every_index(tmp_path):
    case = SCENARIOS["files"]["tau"]
    home = tmp_path / "home"
    stage_file_source(home, case)
    indexes = [home / p for p in case["metadata"]]
    before = [file_stamp(p) for p in indexes]
    orphan = home / case["orphan"]["to"]
    shutil.copyfile(ROOT / case["orphan"]["from"], orphan)
    indexed = {
        Path(json.loads(line)["path"]).name
        for path in indexes
        for line in path.read_text(encoding="utf-8").splitlines()
    }
    assert orphan.is_file() and orphan.name not in indexed
    assert [file_stamp(p) for p in indexes] == before


def test_opencode_database_change_signal_covers_same_seq_settlement(tmp_path):
    case = SCENARIOS["opencode"]
    source = tmp_path / "opencode.db"
    with closing(sqlite3.connect(source)) as writer:
        assert writer.execute("PRAGMA journal_mode=WAL").fetchone() == ("wal",)
        writer.execute("PRAGMA wal_autocheckpoint=0")
        apply_sql(writer, *case["sqlite"])
        with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as reader:
            def data_version():
                return reader.execute("PRAGMA data_version").fetchone()[0]

            def high_water():
                return reader.execute(
                    "SELECT max(seq) FROM session_message WHERE session_id=?",
                    (case["session_id"],),
                ).fetchone()[0]

            def completion():
                return reader.execute(
                    "SELECT json_extract(data,'$.time.completed') FROM session_message WHERE id=?",
                    (case["message_id"],),
                ).fetchone()

            unchanged = data_version()
            seq = high_water()
            assert seq is not None
            assert completion() == (None,)
            assert data_version() == unchanged
            main = file_stamp(source)

            apply_sql(writer, case["settle"])
            assert file_stamp(source) == main
            settled = data_version()
            assert settled != unchanged
            assert high_water() == seq
            assert completion()[0] is not None

            apply_sql(writer, case["delete"])
            assert data_version() != settled
            assert high_water() is None
            assert completion() is None

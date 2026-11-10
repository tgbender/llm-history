"""Validate the synthetic Cursor sources and their expected overlap."""

import hashlib
import json
import sqlite3
from contextlib import closing
from pathlib import Path

CURSOR = Path(__file__).resolve().parent / "cursor"


def field_values(data, number):
    """Read the two protobuf wire types used in this fixture."""
    offset = 0

    def varint():
        nonlocal offset
        value = 0
        for shift in range(0, 70, 7):
            byte = data[offset]
            offset += 1
            value |= (byte & 127) << shift
            if byte < 128:
                return value
        raise ValueError("invalid fixture varint")

    while offset < len(data):
        tag = varint()
        if tag & 7 == 2:
            size = varint()
            end = offset + size
            assert end <= len(data)
            value = data[offset:end]
            offset = end
        else:
            assert tag & 7 == 0
            value = varint()
        if tag >> 3 == number:
            yield value


def first(data, number):
    return next(field_values(data, number))


def load_store(db):
    db.executescript((CURSOR / "store.sql").read_text())
    return metadata(db), blobs(db)


def metadata(db):
    (encoded,) = db.execute("SELECT value FROM meta WHERE key = '0'").fetchone()
    return json.loads(bytes.fromhex(encoded))


def blobs(db):
    return {
        bytes.fromhex(key): data
        for key, data in db.execute("SELECT id, data FROM blobs ORDER BY rowid")
    }


def test_cursor_store_selects_one_conversation_and_matches_transcript(tmp_path):
    with closing(sqlite3.connect(tmp_path / "store.db")) as db:
        meta, stored = load_store(db)
    assert all(hashlib.sha256(data).digest() == key for key, data in stored.items())
    session = meta["agentId"]
    bucket = hashlib.md5(
        meta["workspacePath"].encode(), usedforsecurity=False
    ).hexdigest()
    sidecar = json.loads(
        (CURSOR / "home/chats" / bucket / session / "meta.json").read_text()
    )
    (transcript,) = (CURSOR / "home/projects").glob(
        f"*/agent-transcripts/{session}/{session}.jsonl"
    )
    transcript_rows = [json.loads(line) for line in transcript.read_bytes().splitlines()]
    assert sidecar["cwd"] == meta["workspacePath"]

    refs = list(stored)
    selected = stored[bytes.fromhex(meta["latestRootBlobId"])]
    _, calls_key, *_results, answer_key = field_values(selected, 1)
    assert refs.index(answer_key) < refs.index(calls_key)
    user, calls, failure, success, answer = [
        json.loads(stored[key]) for key in field_values(selected, 1)
    ]
    assert [row["role"] for row in transcript_rows] == ["user", "assistant", "assistant"]
    assert transcript_rows[0]["message"]["content"] == user["content"]
    assert transcript_rows[2]["message"]["content"] == answer["content"]

    calls = calls["content"]
    assert transcript_rows[1]["message"]["content"] == [
        {"type": "tool_use", "name": call["toolName"], "input": call["args"]}
        for call in calls
    ]
    failure, success = failure["content"][0], success["content"][0]
    assert [failure["toolCallId"], success["toolCallId"]] == [
        c["toolCallId"] for c in reversed(calls)
    ]
    assert failure["isError"] is True
    assert (success["isError"], success["result"]) == (False, "amber")

    # There is another root with the same prompt and a conflicting result for
    # the same call ID. An all-rows query would accidentally index both branches.
    (superseded,) = [
        data
        for data in stored.values()
        if data != selected
        and not data.startswith(b"{")
        and list(field_values(data, 22)) == [b"cli"]
    ]
    old_user, _, old_result, old_answer = field_values(superseded, 1)
    assert old_user == first(selected, 1)
    abandoned = json.loads(stored[old_result])["content"][0]
    assert abandoned["toolCallId"] == success["toolCallId"]
    assert abandoned["result"] != success["result"]
    assert json.loads(stored[old_answer])["content"][0]["text"].startswith(
        "Abandoned reply:"
    )

    # The native turn graph shares the prompt and agrees with the selected text.
    turn = first(stored[first(selected, 8)], 1)
    assert first(stored[first(turn, 1)], 25) == sidecar["createdAtMs"]
    *tool_steps, final = [stored[key] for key in field_values(turn, 2)]
    assert [first(first(step, 2), 57).decode() for step in tool_steps] == [
        c["toolCallId"] for c in calls
    ]
    assert first(first(final, 1), 1).decode() == answer["content"][0]["text"]


def test_cursor_missing_result_leaves_selected_root_and_other_messages(tmp_path):
    with closing(sqlite3.connect(tmp_path / "store.db")) as db:
        meta, before = load_store(db)
        db.executescript((CURSOR / "drop-active-result.sql").read_text())
        assert metadata(db) == meta
        after = blobs(db)

    root = bytes.fromhex(meta["latestRootBlobId"])
    refs = list(field_values(after[root], 1))
    assert before[root] == after[root]
    assert before.keys() - after.keys() == {refs[2]}
    assert json.loads(before[refs[2]])["content"][0]["isError"] is True
    assert all(before[key] == after[key] for key in refs if key != refs[2])

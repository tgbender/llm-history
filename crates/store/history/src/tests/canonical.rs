use crate::Store;
use anyhow::Result;
use llm_history_types::Harness;
use llm_history_types::query::Filter;
use llm_history_types::storage::StoreOptions;
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

fn write(path: &Path, records: &[Value]) -> Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(
        path,
        records.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )?;
    Ok(())
}
fn count(store: &Store, table: &str) -> Result<i64> {
    Ok(store.database.connection().query_row(
        &format!("SELECT count(*) FROM {table}"),
        [],
        |r| r.get(0),
    )?)
}

#[test]
fn typed_records_epochs_relations_and_context_survive_append_and_reprocess() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    fs::create_dir_all(&root)?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    let path = root.join("sessions/child.jsonl");
    write(
        &path,
        &[
            json!({"type":"session_meta","timestamp":"2026-10-27T00:00:00Z","payload":{"id":"child","session_id":"runtime","cwd":"/work","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent","agent_role":"explorer","agent_nickname":"Ada"}}}}}),
            // Copied metadata cannot change the owner of a fork's transcript.
            json!({"type":"session_meta","payload":{"id":"parent","cwd":"/wrong"}}),
            json!({"type":"response_item","payload":{"id":"u1","type":"message","role":"user","content":"Inspect this"}}),
            json!({"type":"response_item","payload":{"type":"mcp_tool_call","name":"Read","call_id":"call","arguments":{"file":"a"}}}),
            json!({"type":"response_item","payload":{"type":"mcp_tool_call_output","call_id":"call","output":"contents"}}),
            json!({"type":"compacted","payload":{"message":"summary"}}),
            json!({"type":"response_item","payload":{"id":"a1","type":"message","role":"assistant","phase":"final_answer","content":"Finished"}}),
        ],
    )?;
    let update = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(update.errors.is_empty(), "{update:?}");
    assert_eq!(count(&store, "canonical_events")?, 7);
    assert_eq!(count(&store, "canonical_tool_calls")?, 1);
    assert_eq!(count(&store, "canonical_tool_results")?, 1);
    assert_eq!(count(&store, "canonical_compactions")?, 1);
    assert_eq!(count(&store, "history_context_epochs")?, 2);
    assert_eq!(count(&store, "history_sessions")?, 1);
    let values: (String, String, String) = store.database.connection().query_row(
        "SELECT native_runtime_id,session_kind,agent_label FROM canonical_sessions",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(values, ("runtime".into(), "subagent".into(), "Ada".into()));
    let authorship: String = store.database.connection().query_row(
        "SELECT authorship FROM canonical_messages WHERE protocol_role='user'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(authorship, "agent");
    write(
        &root.join("sessions/parent.jsonl"),
        &[json!({"type":"session_meta","payload":{"id":"parent","cwd":"/work"}})],
    )?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(
        count(&store, "history_session_relations WHERE is_resolved")?,
        1
    );
    assert_eq!(
        count(&store, "canonical_conversation_groups WHERE active")?,
        1
    );
    let filter = Filter {
        include_subagents: true,
        include_agent_protocol: true,
        ..Default::default()
    };
    let before = store.messages_page(&filter, None, usize::MAX)?;
    let mut f = fs::OpenOptions::new().append(true).open(&path)?;
    writeln!(
        f,
        "{}",
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":"After append"}})
    )?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(count(&store, "history_context_epochs")?, 3); // parent initial + child's two epochs
    let messages = store.messages_page(&filter, None, usize::MAX)?;
    assert_eq!(store.context(before[1].message_no, 1, 1, &[])?.len(), 3);
    let result = crate::tests::fixtures::replay(&store, &[], None, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(
        store
            .messages_page(&filter, None, usize::MAX)?
            .iter()
            .map(|m| &m.id)
            .collect::<Vec<_>>(),
        messages.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
    assert_eq!(count(&store, "canonical_events")?, 9);
    assert_eq!(
        count(
            &store,
            "history_event_branch((SELECT max(id) FROM canonical_events))"
        )?,
        1
    );
    Ok(())
}

#[test]
fn claude_blocks_share_one_event_and_keep_outer_uuid_relationships() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("claude");
    fs::create_dir_all(&root)?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    store.add_profile("personal", Harness::Claude, &root, None, None)?;
    write(
        &root.join("projects/p/session.jsonl"),
        &[
            json!({"type":"user","sessionId":"same","uuid":"u","message":{"content":"Question"}}),
            json!({"type":"assistant","sessionId":"same","uuid":"a","parentUuid":"u","message":{"id":"api-id","content":[{"type":"text","text":"Checking"},{"type":"tool_use","id":"c1","name":"Read","input":{"path":"x"}},{"type":"tool_use","id":"c2","name":"Read","input":{"path":"y"}}]}}),
            json!({"type":"user","sessionId":"same","uuid":"r","parentUuid":"a","message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"x"},{"type":"tool_result","tool_use_id":"c2","content":"y"}]}}),
            json!({"type":"assistant","sessionId":"same","uuid":"b","parentUuid":"r","message":{"id":"api-id","stop_reason":"end_turn","content":"Answer"}}),
        ],
    )?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(count(&store, "canonical_events")?, 4);
    assert_eq!(count(&store, "canonical_messages")?, 3);
    assert_eq!(count(&store, "canonical_tool_calls")?, 2);
    assert_eq!(count(&store, "canonical_tool_results")?, 2);
    assert_eq!(count(&store, "canonical_event_relations")?, 3);
    let leaf: i64 = store.database.connection().query_row(
        "SELECT id FROM canonical_events WHERE native_item_id='b'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count(&store, &format!("history_event_branch({leaf})"))?, 4);
    Ok(())
}

#[test]
fn archived_move_reuses_source_and_ids_but_overlapping_live_captures_remain_distinct() -> Result<()>
{
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    fs::create_dir_all(&root)?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    let path = root.join("sessions/rollout-id.jsonl");
    write(
        &path,
        &[
            json!({"type":"session_meta","payload":{"id":"thread"}}),
            json!({"type":"response_item","payload":{"id":"m","type":"message","role":"user","content":"same"}}),
        ],
    )?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let before = store.messages_page(&Filter::default(), None, usize::MAX)?;
    fs::create_dir_all(root.join("archived_sessions"))?;
    fs::rename(&path, root.join("archived_sessions/rollout-id.jsonl"))?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    assert_eq!(count(&store, "sources")?, 1);
    assert_eq!(
        store.messages_page(&Filter::default(), None, usize::MAX)?[0].id,
        before[0].id
    );
    fs::create_dir_all(path.parent().unwrap())?;
    fs::copy(root.join("archived_sessions/rollout-id.jsonl"), &path)?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    assert_eq!(count(&store, "sources")?, 2);
    assert_eq!(count(&store, "canonical_sessions")?, 1);
    assert_eq!(count(&store, "canonical_messages")?, 2);
    assert_eq!(count(&store, "canonical_session_sources")?, 2);
    assert_eq!(
        count(
            &store,
            "(SELECT DISTINCT session_seq FROM canonical_events)"
        )?,
        4
    );
    Ok(())
}

#[test]
fn late_metadata_and_replay_keep_one_owner_and_stable_ids() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("claude");
    fs::create_dir_all(&root)?;
    let db = dir.path().join("history.duckdb");
    let store = Store::open_with_options(
        &db,
        false,
        StoreOptions {
            batch_records: 1,
            ..StoreOptions::default()
        },
    )?;
    store.add_profile("work", Harness::Claude, &root, None, None)?;
    write(
        &root.join("projects/p/arbitrary-name.jsonl"),
        &[
            json!({"type":"mode","mode":"default"}),
            json!({"type":"system","subtype":"compact_boundary"}),
            json!({"type":"user","sessionId":"real-id","uuid":"u","message":{"content":"Question"}}),
        ],
    )?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(count(&store, "canonical_sessions WHERE active")?, 1);
    assert_eq!(count(&store, "canonical_events")?, 3);
    assert_eq!(
        count(
            &store,
            "canonical_events \
             WHERE session_id=(\
             SELECT id \
             FROM canonical_sessions \
             WHERE native_session_id='claude:real-id')"
        )?,
        3
    );
    let question_epoch = |store: &Store| -> Result<i64> {
        Ok(store.database.connection().query_row(
            "SELECT ep.ordinal-css.sequence_base
             FROM events e JOIN canonical_context_epochs ep ON ep.id=e.context_epoch_id
             JOIN canonical_session_sources css ON css.session_id=e.session_id AND css.source_id=e.source_id
             WHERE e.text='Question' AND e.active AND e.fact_index=0",
            [],
            |r| r.get(0),
        )?)
    };
    assert_eq!(question_epoch(&store)?, 1);
    let before = store.messages_page(&Filter::default(), None, usize::MAX)?;
    fs::remove_dir_all(root)?;
    drop(store);
    let store = Store::open(&db, false)?;
    assert!(
        crate::tests::fixtures::replay(&store, &[], None, &[], false)?
            .errors
            .is_empty()
    );
    assert_eq!(count(&store, "canonical_sessions WHERE active")?, 1);
    assert_eq!(count(&store, "canonical_events")?, 3);
    assert_eq!(question_epoch(&store)?, 1);
    assert_eq!(
        store.messages_page(&Filter::default(), None, usize::MAX)?[0].id,
        before[0].id
    );
    assert_eq!(
        store.messages_page(&Filter::default(), None, usize::MAX)?[0].text,
        "Question"
    );
    Ok(())
}

#[test]
fn changing_a_parent_splits_and_rejoins_conversations_within_the_profile() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    fs::create_dir_all(&root)?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    for id in ["parent", "child", "unrelated"] {
        write(
            &root.join(format!("sessions/{id}.jsonl")),
            &[json!({"type":"session_meta","payload":{"id":id}})],
        )?;
    }
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    assert_eq!(
        count(&store, "canonical_conversation_groups WHERE active")?,
        3
    );
    let unrelated = |store: &Store| -> Result<(String, String)> {
        Ok(store.database.connection().query_row(
            "SELECT m.updated_at::VARCHAR,g.updated_at::VARCHAR \
             FROM canonical_conversation_memberships m \
             JOIN canonical_conversation_groups g ON g.id=m.conversation_group_id \
             JOIN sessions s ON s.id=m.session_id \
             WHERE s.native_id='unrelated'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    };
    let untouched = unrelated(&store)?;
    let child = root.join("sessions/child.jsonl");
    write(
        &child,
        &[json!({"type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}})],
    )?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(
        count(&store, "canonical_conversation_groups WHERE active")?,
        2
    );
    assert_eq!(unrelated(&store)?, untouched);
    write(
        &child,
        &[json!({"type":"session_meta","payload":{"id":"child"}})],
    )?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(
        count(&store, "canonical_conversation_groups WHERE active")?,
        3
    );
    assert_eq!(unrelated(&store)?, untouched);
    assert_eq!(
        count(&store, "canonical_session_relations WHERE active")?,
        0
    );
    Ok(())
}

#[test]
fn replacing_session_ownership_retires_old_conversation_and_epochs() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    fs::create_dir_all(&root)?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    for id in ["old", "new"] {
        write(
            &root.join("sessions/a.jsonl"),
            &[
                json!({"type":"session_meta","payload":{"id":id}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":id}}),
            ],
        )?;
        let update = crate::tests::fixtures::capture(&store, &[], false)?;
        assert!(update.errors.is_empty(), "{update:?}");
    }
    assert_eq!(
        count(&store, "canonical_conversation_groups WHERE active")?,
        1
    );
    assert_eq!(
        count(
            &store,
            "canonical_conversation_memberships m \
             JOIN canonical_sessions s ON s.id=m.session_id \
             WHERE NOT s.active"
        )?,
        0
    );
    assert_eq!(
        count(
            &store,
            "canonical_context_epochs ep \
             JOIN canonical_sessions s ON s.id=ep.session_id \
             WHERE ep.active AND NOT s.active"
        )?,
        0
    );
    Ok(())
}

#[test]
fn native_parent_aliases_runtime_ids_and_guardian_protocol_follow_python() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    fs::create_dir_all(&root)?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    for (id, payload) in [
        ("parent", json!({"id":"parent"})),
        ("fork", json!({"id":"fork","parent_session_id":"parent"})),
        (
            "agent",
            json!({"id":"agent","forked_from_id":"parent","thread_source":"subagent"}),
        ),
    ] {
        write(
            &root.join(format!("sessions/{id}.jsonl")),
            &[
                json!({"type":"session_meta","payload":payload}),
                json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":"{\"risk_level\":\"high\",\"user_authorization\":\"allowed\",\"outcome\":\"protocol\"}"}}),
            ],
        )?;
    }
    // An approval review: a quoted history in, a verdict out. Both are protocol.
    write(
        &root.join("sessions/guardian.jsonl"),
        &[
            json!({"type":"session_meta","payload":{"id":"guardian","thread_source":"subagent","source":{"subagent":{"other":"guardian"}}}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"The following is the Codex agent history"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":"{\"outcome\":\"allow\"}"}}),
        ],
    )?;
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");
    assert_eq!(
        count(
            &store,
            "canonical_sessions WHERE native_runtime_id=native_session_id"
        )?,
        4
    );
    let guardian = store.list_sessions(&Filter::default(), 10, false)?;
    assert!(guardian.iter().all(|s| s.thread_id != "guardian"));
    let sessions = store.list_sessions(
        &Filter {
            include_subagents: true,
            ..Default::default()
        },
        10,
        false,
    )?;
    let guardian = sessions.iter().find(|s| s.thread_id == "guardian").unwrap();
    assert_eq!(guardian.session_kind, "guardian");
    assert_eq!(guardian.role, "guardian");
    assert_eq!(guardian.title, "Guardian review");
    assert_eq!(guardian.relation, "");
    assert_eq!(
        count(
            &store,
            "canonical_session_relations WHERE relation_kind='forked_from' AND active"
        )?,
        1
    );
    assert_eq!(
        count(
            &store,
            "canonical_session_relations WHERE relation_kind='spawned' AND active"
        )?,
        1
    );
    // Naming a kind reaches sessions and protocol that are hidden by default.
    let kind = |kind: &str| Filter {
        kind: Some(kind.into()),
        ..Default::default()
    };
    let listed = store.list_sessions(&kind("guardian"), 10, false)?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].thread_id, "guardian");
    let reviews = store.messages_page(&kind("guardian"), None, usize::MAX)?;
    assert_eq!(reviews.len(), 2);
    assert!(reviews.iter().all(|m| m.thread_id == "guardian"));
    let delegated = store.messages_page(&kind("subagent"), None, usize::MAX)?;
    assert_eq!(delegated.len(), 1);
    assert_eq!(delegated[0].thread_id, "agent");
    assert_eq!(
        store
            .messages_page(&kind("interactive"), None, usize::MAX)?
            .len(),
        2
    );
    let mut f = Filter {
        include_subagents: true,
        ..Default::default()
    };
    assert_eq!(store.messages_page(&f, None, usize::MAX)?.len(), 2);
    f.include_agent_protocol = true;
    assert_eq!(store.messages_page(&f, None, usize::MAX)?.len(), 5);
    assert!(
        !store
            .pending_messages("test", &[], None, 0, 100, 10000, &[])?
            .messages
            .iter()
            .any(|m| m.thread_id == "agent" || m.thread_id == "guardian")
    );
    let result = crate::tests::fixtures::replay(&store, &[], None, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    f.include_agent_protocol = false;
    assert_eq!(store.messages_page(&f, None, usize::MAX)?.len(), 2);
    Ok(())
}

#[test]
fn tool_result_hash_contract_is_preserved_across_reprocessing() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    fs::create_dir_all(&root)?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    write(
        &root.join("sessions/a.jsonl"),
        &[
            json!({"type":"session_meta","payload":{"id":"thread"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"hello"}}),
        ],
    )?;
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");
    for _ in 0..2 {
        let hash: String = store.database.connection().query_row(
            "SELECT output_hash FROM canonical_tool_results",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let result = crate::tests::fixtures::replay(&store, &[], None, &[], false)?;
        assert!(result.errors.is_empty(), "{result:?}");
    }
    Ok(())
}

#[test]
fn rewriting_the_earliest_file_moves_a_multi_file_session_start() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    fs::create_dir_all(&root)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    let rollout = |day: &str, text: &str| {
        let ts = format!("2026-10-{day}T00:00:00Z");
        vec![
            json!({"type":"session_meta","timestamp":ts,"payload":{"id":"s","timestamp":ts,"cwd":"/work"}}),
            json!({"type":"response_item","timestamp":ts,"payload":{"type":"message","role":"user","content":text}}),
        ]
    };
    let started = |store: &Store| -> Result<String> {
        Ok(store.database.connection().query_row(
            "SELECT strftime(started_at_utc,'%Y-%m-%d') \
             FROM canonical_sessions \
             WHERE native_session_id='s' AND active",
            [],
            |r| r.get(0),
        )?)
    };
    write(&root.join("sessions/a.jsonl"), &rollout("01", "first"))?;
    write(&root.join("sessions/b.jsonl"), &rollout("05", "second"))?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!(started(&store)?, "2026-10-01");
    // Only the earliest file is rebuilt; its old start must not survive.
    write(
        &root.join("sessions/a.jsonl"),
        &rollout("07", "first, rewritten"),
    )?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
    assert_eq!(started(&store)?, "2026-10-05");
    Ok(())
}

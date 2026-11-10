use crate::Store;
use anyhow::Result;
use llm_history_types::Harness;
use llm_history_types::query::Filter;
use serde_json::json;
use std::{fs, io::Write};

#[test]
fn source_timezone_and_complete_context_and_token_facts_survive_resume() -> Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    fs::create_dir_all(root.join("sessions"))?;
    let path = root.join("sessions/a.jsonl");
    let records = [
        json!({"type":"session_meta","payload":{"id":"native","cwd":"/work"}}),
        json!({"type":"turn_context","timestamp":"2026-07-01T09:30:00","payload":{"timezone":"America/Denver","model":"model","effort":"high","turn_id":"turn","current_date":"2026-07-01","approval_policy":"never","sandbox_policy":{"type":"workspace-write"},"service_tier":"fast","personality":"direct","workspace_roots":["/work"],"collaboration_mode":{"mode":"plan"}}}),
        json!({"type":"response_item","timestamp":"2026-07-01T09:31:00","payload":{"type":"message","role":"user","content":"question"}}),
        json!({"type":"response_item","timestamp":"2026-07-01T09:31:10","payload":{"type":"function_call","call_id":"c","name":"read_file","arguments":"{}"}}),
        json!({"type":"response_item","timestamp":"2026-07-01T09:31:20","payload":{"type":"function_call_output","call_id":"c","output":"hello"}}),
        json!({"type":"event_msg","timestamp":"2026-07-01T09:32:00","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":3},"last_token_usage":{"input_tokens":10,"output_tokens":3},"model_context_window":128000},"rate_limits":{"limit_id":"api"}}}),
    ];
    fs::write(
        &path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )?;
    let db = d.path().join("history.duckdb");
    let store = Store::open(&db, false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    let r = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(r.errors.is_empty(), "{r:?}");
    let values:(String,String,String,i32)=store.database.connection().query_row("SELECT approval_policy,sandbox_policy,service_tier,timestamp_offset_minutes FROM turn_contexts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    assert_eq!(
        values,
        (
            "never".into(),
            "workspace-write".into(),
            "fast".into(),
            -360
        )
    );
    assert_eq!(
        store
            .messages_page(
                &Filter {
                    since: Some("2026-07-01T15:30:00Z".into()),
                    ..Default::default()
                },
                None,
                usize::MAX
            )?
            .len(),
        1
    );
    assert_eq!(
        store
            .messages_page(
                &Filter {
                    until: Some("2026-07-01T15:30:00Z".into()),
                    ..Default::default()
                },
                None,
                usize::MAX
            )?
            .len(),
        0
    );
    let wrong: i64 = store.database.connection().query_row(
        "SELECT count(*) \
         FROM (\
         SELECT event_id,timestamp_utc \
         FROM canonical_tool_calls \
         UNION ALL \
         SELECT event_id,timestamp_utc \
         FROM canonical_tool_results) t \
         JOIN canonical_events e ON e.id=t.event_id \
         WHERE t.timestamp_utc IS DISTINCT FROM e.timestamp_utc",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(wrong, 0);
    drop(store);
    writeln!(
        fs::OpenOptions::new().append(true).open(&path)?,
        "{}",
        json!({"type":"event_msg","timestamp":"2026-07-01T09:33:00","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":35,"output_tokens":7},"model_context_window":128000}}})
    )?;
    let store = Store::open(&db, false)?;
    let r = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(r.errors.is_empty(), "{r:?}");
    let values: (i64, i64, Option<i64>, i64) = store.database.connection().query_row(
        "SELECT token_ordinal,context_line_no,last_input_tokens,\
         total_input_tokens \
         FROM token_events \
         ORDER BY line_no DESC \
         LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    assert_eq!(values, (2, 2, None, 35));
    let usage = store.usage(&Filter::default(), "UTC", false, false, 100)?;
    assert_eq!(usage[0].input, 10);
    assert_eq!(usage[0].output, 3);
    Ok(())
}
#[test]
fn repair_recaptures_live_edits_and_reuses_unchanged_generation_and_ids() -> Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    fs::create_dir_all(root.join("sessions"))?;
    let path = root.join("sessions/a.jsonl");
    let text = "OLDmiddle conversation";
    fs::write(
        &path,
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"s"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":text}})
        ),
    )?;
    let store = Store::open(&d.path().join("db.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let id = store.messages_page(&Filter::default(), None, usize::MAX)?[0]
        .id
        .clone();
    assert!(
        crate::tests::fixtures::capture(&store, &[], true)?
            .errors
            .is_empty()
    );
    assert_eq!(
        store.messages_page(&Filter::default(), None, usize::MAX)?[0].id,
        id
    );
    assert_eq!(
        store
            .database
            .connection()
            .query_row("SELECT count(*) FROM generations", [], |r| r
                .get::<_, i64>(0))?,
        1
    );
    assert!(
        store.messages_page(&Filter::default(), None, usize::MAX)?[0]
            .text
            .contains("OLDmiddle")
    );
    let body = fs::read_to_string(&path)?.replace("OLDmiddle", "NEWmiddle");
    fs::write(&path, body)?;
    let repair = crate::tests::fixtures::capture(&store, &[], true)?;
    assert!(repair.errors.is_empty(), "{repair:?}");
    assert!(
        store.messages_page(&Filter::default(), None, usize::MAX)?[0]
            .text
            .contains("NEWmiddle")
    );
    assert_eq!(
        store
            .database
            .connection()
            .query_row("SELECT count(*) FROM generations", [], |r| r
                .get::<_, i64>(0))?,
        2
    );
    Ok(())
}

/// Leaves stale derived contents for explicit replay.
fn age(store: &Store) -> Result<()> {
    store.database.connection().execute_batch(
        "UPDATE events \
         SET session_seq=NULL,timestamp_utc=NULL,context_epoch_id=NULL,\
         turn_id=NULL; \
         DELETE FROM provider_turn_contexts; \
         UPDATE provider_token_events \
         SET total_input_tokens=last_input_tokens; \
         UPDATE canonical_sessions \
         SET started_at_utc='2000-01-01',last_event_at_utc='2000-01-01'",
    )?;
    Ok(())
}

#[test]
fn replay_rebuilds_provider_facts_and_session_dates_from_the_archive() -> Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    fs::create_dir_all(root.join("sessions"))?;
    fs::write(root.join("sessions/a.jsonl"), [
        json!({"type":"session_meta","payload":{"id":"s","timestamp":"2026-07-01T09:30:00"}}),
        json!({"type":"turn_context","timestamp":"2026-07-01T09:30:00","payload":{"timezone":"America/Denver"}}),
        json!({"type":"response_item","timestamp":"2026-07-01T09:31:00","payload":{"type":"message","role":"user","content":"question"}}),
    ].iter().map(|v| format!("{v}\n")).collect::<String>())?;
    let db = d.path().join("history.duckdb");
    let store = Store::open(&db, false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    let id = store.messages_page(&Filter::default(), None, usize::MAX)?[0]
        .id
        .clone();
    age(&store)?;
    drop(store);
    let store = Store::open(&db, false)?;
    assert!(
        crate::tests::fixtures::replay(&store, &[], None, &[], false)?
            .errors
            .is_empty()
    );
    let message = store
        .messages_page(&Filter::default(), None, usize::MAX)?
        .remove(0);
    assert_eq!(message.id, id);
    assert_eq!(
        message.timestamp_utc.as_deref(),
        Some("2026-07-01T15:31:00.000Z")
    );
    let dates: (String, String) = store.database.connection().query_row(
        "SELECT strftime(started_at_utc,'%H:%M'),\
         strftime(last_event_at_utc,'%H:%M') \
         FROM canonical_sessions \
         WHERE active",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(dates, ("15:30".into(), "15:31".into()));
    let contexts: i64 = store.database.connection().query_row(
        "SELECT count(*) FROM provider_turn_contexts",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(contexts, 1);
    let status = store.status(&["work".into()], None, llm_history_ingest::PARSER_VERSION)?;
    assert_eq!(status["messages"], 1);
    assert_eq!(
        status["message_max_timestamp_utc"],
        "2026-07-01 15:31:00+00"
    );
    Ok(())
}

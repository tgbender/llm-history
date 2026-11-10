use crate::Store;
use anyhow::Result;
use llm_history_types::Harness;
use llm_history_types::query::Filter;

#[test]
fn archived_reprocessing_repairs_facts_with_stable_ids_without_original_files() -> Result<()> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("reprocess.duckdb");
    let root = d.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    std::fs::write(
        root.join("sessions/a.jsonl"),
        include_bytes!("../../../../../fixtures/codex.jsonl"),
    )?;
    let store = Store::open(&db, false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    let original = store.messages_page(&Filter::default(), None, usize::MAX)?;
    let source: i64 = store
        .database
        .connection()
        .query_row("select id from sources", [], |r| r.get(0))?;
    std::fs::remove_dir_all(root)?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    store.database.connection().execute_batch(
        "update events \
         set text='broken projection',text_hash='broken' \
         where kind='message';\
         update lexical_documents \
         set body='broken projection',aliases='';",
    )?;
    drop(store);
    let before = std::fs::read(&db)?;
    let reader = Store::open(&db, true)?;
    let receipt = crate::tests::fixtures::replay(
        &reader,
        &["personal".into()],
        Some("codex"),
        &[source],
        true,
    )?;
    assert_eq!(receipt.sources, 1);
    assert!(receipt.errors.is_empty());
    drop(reader);
    assert_eq!(std::fs::read(&db)?, before);
    let store = Store::open(&db, false)?;
    let receipt =
        crate::tests::fixtures::replay(&store, &["personal".into()], None, &[source], false)?;
    assert!(receipt.errors.is_empty(), "{receipt:?}");
    let repaired = store.messages_page(&Filter::default(), None, usize::MAX)?;
    assert_eq!(
        repaired
            .iter()
            .map(|m| (&m.id, &m.text, &m.text_hash))
            .collect::<Vec<_>>(),
        original
            .iter()
            .map(|m| (&m.id, &m.text, &m.text_hash))
            .collect::<Vec<_>>()
    );
    assert_eq!(store.verify()?["generations"], 1);
    assert!(
        store
            .database
            .connection()
            .query_row("select missing from sources", [], |r| r.get::<_, bool>(0))?
    );
    Ok(())
}
#[test]
fn failed_reprocessing_rolls_back_and_parser_upgrades_replay_before_append() -> Result<()> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("reprocess.duckdb");
    let root = d.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    std::fs::write(
        root.join("sessions/a.jsonl"),
        include_bytes!("../../../../../fixtures/codex.jsonl"),
    )?;
    let store = Store::open(&db, false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    let original = store.messages_page(&Filter::default(), None, usize::MAX)?;
    // An undecodable payload fails the replay; `raw verify` owns integrity checks.
    store.database.connection().execute_batch(
        "update events \
         set text='keep on failure' \
         where kind='message';\
         create table sizes as \
         select hash,raw_size \
         from native_documents;\
         update native_documents \
         set raw_size=1",
    )?;
    let session_metadata = |store: &Store| -> Result<serde_json::Value> {
        Ok(store.database.connection().query_row(
            "SELECT started_at_utc::VARCHAR,last_event_at_utc::VARCHAR,metadata_json,active,updated_at::VARCHAR FROM canonical_sessions",
            [],
            |r| Ok(serde_json::json!([
                r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?, r.get::<_, bool>(3)?, r.get::<_, String>(4)?
            ])),
        )?)
    };
    let session_before = session_metadata(&store)?;
    let receipt = crate::tests::fixtures::replay(&store, &[], None, &[], false)?;
    assert_eq!(receipt.errors.len(), 1);
    let session_after = session_metadata(&store)?;
    assert_eq!(
        session_after, session_before,
        "failed replay changed session metadata"
    );
    assert!(
        store
            .messages_page(&Filter::default(), None, usize::MAX)?
            .iter()
            .all(|m| m.text == "keep on failure")
    );
    store.database.connection().execute_batch(
        "update native_documents p set raw_size=s.raw_size from sizes s where s.hash=p.hash",
    )?;
    store
        .database
        .connection()
        .execute_batch("update sources set parser_version=0")?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    assert_eq!(
        store
            .messages_page(&Filter::default(), None, usize::MAX)?
            .iter()
            .map(|m| (&m.id, &m.text))
            .collect::<Vec<_>>(),
        original
            .iter()
            .map(|m| (&m.id, &m.text))
            .collect::<Vec<_>>()
    );
    assert_eq!(store.verify()?["generations"], 1);
    assert!(
        crate::tests::fixtures::replay(
            &store,
            &["personal".into()],
            Some("claude"),
            &[original[0].id.parse()?],
            false
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn replay_keeps_non_object_json_and_invalid_lines_archive_only() -> Result<()> {
    use serde_json::json;
    for harness in [Harness::Codex, Harness::Claude] {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join(harness.key());
        let path = root.join(if harness == Harness::Codex {
            "sessions/a.jsonl"
        } else {
            "projects/p/a.jsonl"
        });
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut bytes = String::from("[]\nnull\n42\nfalse\n\"text\"\n{broken\n\n");
        for record in llm_history_core::fixture_record(harness, "s", "keep me") {
            bytes.push_str(&format!("{record}\n"));
        }
        std::fs::write(&path, &bytes)?;
        let store = Store::open(&dir.path().join("history.duckdb"), false)?;
        store.add_profile("work", harness, &root, None, None)?;
        let update = crate::tests::fixtures::capture(&store, &[], false)?;
        assert!(update.errors.is_empty(), "{update:?}");
        let snapshot = |store: &Store| -> Result<Vec<serde_json::Value>> {
            Ok(store.database.connection().prepare(
                "SELECT id,ordinal,fact_index,session_seq,context_epoch_id,kind,text,text_hash FROM events WHERE active ORDER BY ordinal,fact_index",
            )?.query_map([], |r| Ok(json!([
                r.get::<_, i64>(0)?,r.get::<_, u64>(1)?,r.get::<_, i64>(2)?,
                r.get::<_, Option<i64>>(3)?,r.get::<_, Option<i64>>(4)?,
                r.get::<_, String>(5)?,r.get::<_, String>(6)?,r.get::<_, String>(7)?
            ])))?.collect::<duckdb::Result<_>>()?)
        };
        let before = snapshot(&store)?;
        let dry_run = crate::tests::fixtures::replay(&store, &[], None, &[], true)?;
        assert!(dry_run.errors.is_empty(), "{dry_run:?}");
        assert_eq!(dry_run.facts.values().sum::<usize>(), before.len());
        let replay = crate::tests::fixtures::replay(&store, &[], None, &[], false)?;
        assert!(replay.errors.is_empty(), "{replay:?}");
        assert_eq!(
            snapshot(&store)?,
            before,
            "{} replay diverged",
            harness.key()
        );
        assert_eq!(replay.facts, dry_run.facts);
        let source = store.raw_sources(&Default::default())?.remove(0);
        let mut exported = Vec::new();
        store.export_raw(source.id, None, &mut exported)?;
        assert_eq!(exported, bytes.as_bytes());
        assert_eq!(store.verify()?["verified"], json!(true));
        assert_eq!(
            store.messages_page(&Filter::default(), None, usize::MAX)?[0].text,
            "keep me"
        );
    }
    Ok(())
}

#[test]
fn a_failed_source_keeps_its_dates_while_other_sources_in_the_batch_rebuild() -> Result<()> {
    use serde_json::json;
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    for name in ["bad", "good"] {
        let mut records = llm_history_core::fixture_record(Harness::Codex, name, name);
        records[0]["payload"]["timestamp"] = json!("2026-10-01T00:00:00Z");
        std::fs::write(
            root.join(format!("sessions/{name}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let started = |store: &Store| -> Result<Option<String>> {
        Ok(store.database.connection().query_row(
            "SELECT started_at_utc::VARCHAR FROM canonical_sessions WHERE native_session_id='bad'",
            [],
            |r| r.get(0),
        )?)
    };
    let before = started(&store)?;
    assert!(before.is_some());
    store.database.connection().execute_batch(
        "UPDATE native_documents SET raw_size=1 WHERE hash IN (\
         SELECT r.payload_hash FROM native_observations r \
         JOIN generations g ON g.id=r.generation_id \
         JOIN sources s ON s.id=g.source_id WHERE s.source_key='sessions/bad.jsonl');\
         UPDATE events SET text='stale' WHERE kind='message' AND session_id IN (\
         SELECT id FROM sessions WHERE native_id='good')",
    )?;
    let replay = crate::tests::fixtures::replay(&store, &[], None, &[], false)?;
    assert_eq!(replay.errors.len(), 1, "{replay:?}");
    assert_eq!(replay.sources, 1);
    assert_eq!(started(&store)?, before);
    let messages = store.messages_page(&Filter::default(), None, usize::MAX)?;
    assert!(messages.iter().any(|m| m.text == "good"));
    assert!(messages.iter().any(|m| m.text == "bad"));
    Ok(())
}

use crate::Store;
const SQL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(llm_history_types::storage::DEFAULT_SQL_TIMEOUT_SECONDS);
use anyhow::Result;
use llm_history_types::Harness;
use llm_history_types::archive::RawFilter;
use llm_history_types::query::Filter;
use serde_json::json;

#[test]
fn sql_hashes_match_capture_across_duckdb_chunks() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    let mut statement = store.database.connection().prepare(
        "SELECT bytes,llm_history_blake3(bytes),llm_history_xxh64(bytes)
         FROM (
            SELECT CASE WHEN i%7=0 THEN NULL WHEN i%5=0 THEN from_hex('00fffe')
                        WHEN i%3=0 THEN from_hex('')
                        ELSE encode('unicode 🦀 ' || repeat('x', i%100)) END AS bytes
            FROM range(10000) t(i)
         )",
    )?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let bytes: Option<Vec<u8>> = row.get(0)?;
        let digest: Option<Vec<u8>> = row.get(1)?;
        let fingerprint: Option<u64> = row.get(2)?;
        assert_eq!(
            digest,
            bytes.as_ref().map(|v| blake3::hash(v).as_bytes().to_vec())
        );
        assert_eq!(
            fingerprint,
            bytes.as_ref().map(|v| xxhash_rust::xxh64::xxh64(v, 0))
        );
        count += 1;
    }
    assert_eq!(count, 10_000);
    Ok(())
}

#[test]
fn inspection_scopes_duplicate_threads_and_context_without_losing_archived_sources() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    for (profile, date) in [("work", "2026-10-26"), ("personal", "2026-10-27")] {
        let root = d.path().join(profile);
        std::fs::create_dir_all(root.join("sessions"))?;
        store.add_profile(profile, Harness::Codex, &root, None, None)?;
        let mut records = vec![
            json!({"type":"session_meta","timestamp":format!("{date}T12:00:00Z"),"payload":{"id":"same-thread","cwd":format!("/{profile}")}}),
            json!({"type":"turn_context","timestamp":format!("{date}T12:00:01Z"),"payload":{"model":"example","effort":"high"}}),
        ];
        for text in ["before", "target", "after"] {
            records.push(json!({"type":"response_item","timestamp":format!("{date}T12:01:00Z"),"payload":{"type":"message","role":"assistant","phase":"final_answer","content":text}}));
        }
        std::fs::write(
            root.join("sessions/history.jsonl"),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    crate::tests::fixtures::capture(&store, &[], false)?;
    let sessions = store.list_sessions(&Filter::default(), 30, false)?;
    assert_eq!(sessions[0].profile, "personal");
    assert_eq!(sessions[0].model, "example");
    assert_eq!(sessions[0].effort, "high");
    let selected = Filter {
        profiles: vec!["work".into()],
        ..Default::default()
    };
    let messages = store.messages_page(&selected, None, usize::MAX)?;
    assert_eq!(messages.len(), 3);
    let status = store.status(
        &selected.profiles,
        Some("codex"),
        llm_history_ingest::PARSER_VERSION,
    )?;
    assert_eq!(status["profiles"], 1);
    assert_eq!(status["sources"], 1);
    assert_eq!(status["canonical_messages"], 3);
    let context = store.context(messages[1].message_no, 1, 1, &selected.profiles)?;
    assert_eq!(
        context.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["before", "target", "after"]
    );
    assert!(
        store
            .context(messages[1].message_no, 1, 1, &["personal".into()])
            .is_err()
    );
    let mut filter = RawFilter {
        selector: Some("same-thread".into()),
        latest: true,
        ..Default::default()
    };
    assert_eq!(store.raw_sources(&filter)?.len(), 2);
    filter.profiles = vec!["work".into()];
    let source = store.raw_sources(&filter)?.remove(0);
    let mut exported = Vec::new();
    store.export_raw(source.id, None, &mut exported)?;
    assert_eq!(exported, std::fs::read(&source.path)?);
    assert_eq!(store.verify_raw(&filter, true, false)?["verified"], true);
    std::fs::write(&source.path, b"replacement\n")?;
    assert_eq!(store.verify_raw(&filter, true, false)?["verified"], false);
    std::fs::remove_file(&source.path)?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!(store.verify_raw(&filter, false, false)?["verified"], true);
    assert!(store.raw_sources(&filter)?[0].missing);
    assert_eq!(store.raw_stats_filtered(&filter)?["sources"], 1);
    Ok(())
}

#[test]
fn compaction_preserves_identity_and_sequence_progress_without_archiving() -> Result<()> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("history.duckdb");
    let store = Store::open(&db, false)?;
    let identity = store.identity()?;
    let bundled: i64 = store.database.connection().query_row(
        "SELECT count(*) \
         FROM duckdb_extensions() \
         WHERE extension_name IN ('fts','icu') AND loaded \
         AND install_mode='STATICALLY_LINKED'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(bundled, 2);
    store.create_fts()?;
    store.database.connection().execute_batch(
        "INSERT INTO lexical_documents(id,body,aliases) VALUES (1,'before compaction','');",
    )?;
    store.database.connection().execute_batch(
        "CREATE TABLE extra(id BIGINT PRIMARY KEY, value VARCHAR); \
         INSERT INTO extra \
         VALUES (1,'retained'); \
         CREATE SCHEMA extra_schema; \
         CREATE TABLE extra_schema.imported AS \
         SELECT 42 value; \
         SELECT nextval('entity_ids');",
    )?;
    let previous: i64 =
        store
            .database
            .connection()
            .query_row("SELECT currval('entity_ids')", [], |r| r.get(0))?;
    let result = store.compact()?;
    assert!(result["after_bytes"].as_u64().unwrap() > 0);
    assert!(!db.with_file_name("history.duckdb.archives").exists());
    let store = Store::open(&db, false)?;
    store.database.connection().execute_batch(
        "INSERT INTO lexical_documents(id,body,aliases) VALUES (2,'appended afterward','');",
    )?;
    assert_eq!(store.fts_scores("afterward")?.len(), 1);
    assert_eq!(store.identity()?, identity);
    assert_eq!(
        store
            .database
            .connection()
            .query_row("SELECT value FROM extra", [], |r| r.get::<_, String>(0))?,
        "retained"
    );
    assert_eq!(
        store.database.connection().query_row(
            "SELECT value FROM extra_schema.imported",
            [],
            |r| r.get::<_, i32>(0)
        )?,
        42
    );
    assert!(
        store
            .database
            .connection()
            .query_row("SELECT nextval('entity_ids')", [], |r| r.get::<_, i64>(0))?
            > previous
    );
    Ok(())
}

#[test]
fn sql_byte_bounds_preserve_unicode_and_full_output_is_explicit() -> Result<()> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("history.duckdb");
    drop(Store::open(&db, false)?);
    let store = Store::open(&db, true)?;
    let values = store.sql_bounded("SELECT 1234567890123::BIGINT n, 42::HUGEINT huge, [1,\
                            2] items, {'name':'hello'} details, DATE '2026-10-27' recorded_date, TIME '09:30:00' clock, 1.25::DECIMAL(3,2) price", 1, 2000, 100_000, false, SQL_TIMEOUT)?;
    assert_eq!(
        values["rows"][0],
        json!({"n":1234567890123i64,"huge":42,"items":[1,2],"details":{"name":"hello"},"recorded_date":"2026-10-27","clock":"09:30:00","price":"1.25"})
    );
    let times = store.sql_bounded(
        "SELECT TIMESTAMP '2026-07-01 15:00:00' naive, TIMESTAMPTZ '2026-07-01 15:00:00Z' aware",
        1,
        2000,
        100_000,
        false,
        SQL_TIMEOUT,
    )?;
    assert_eq!(
        times["rows"][0],
        json!({"naive":"2026-07-01 15:00:00","aware":"2026-07-01 15:00:00+00:00"})
    );
    let bounded = store.sql_bounded(
        "SELECT repeat('界',100) body",
        1,
        64,
        256,
        false,
        SQL_TIMEOUT,
    )?;
    assert!(bounded["rows"][0]["body"].as_str().unwrap().len() <= 64);
    assert!(serde_json::to_vec(&bounded)?.len() <= 256);
    assert_eq!(bounded["truncated"], true);
    assert_eq!(bounded["truncation_reasons"], json!(["cell_byte_limit"]));
    assert_eq!(bounded["truncated_cells"], 1);
    let complete = store.sql_bounded("SELECT 'short' body", 1, 64, 100_000, false, SQL_TIMEOUT)?;
    assert_eq!(complete["truncated"], false);
    let bounded = store.sql_bounded(
        "SELECT i FROM range(100) t(i)",
        3,
        2000,
        8000,
        false,
        SQL_TIMEOUT,
    )?;
    assert_eq!(bounded["rows"].as_array().unwrap().len(), 3);
    assert_eq!(bounded["truncated"], true);
    let full = store.sql_bounded(
        "SELECT i FROM range(100) t(i)",
        usize::MAX,
        usize::MAX,
        usize::MAX,
        false,
        SQL_TIMEOUT,
    )?;
    assert_eq!(full["rows"].as_array().unwrap().len(), 100);
    assert_eq!(full["truncated"], false);
    Ok(())
}

#[test]
fn sql_headers_are_bounded_and_report_truncation_without_losing_column_identity() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("history.duckdb");
    drop(Store::open(&db, false)?);
    let store = Store::open(&db, true)?;
    for prefix in ["x".repeat(1000), "界".repeat(1000), "\\\"".repeat(500)] {
        let names = [format!("{prefix}a"), format!("{prefix}b")];
        let query = format!(
            "SELECT 1 AS \"{}\",2 AS \"{}\"",
            names[0].replace('"', "\"\""),
            names[1].replace('"', "\"\"")
        );
        let bounded = store.sql_bounded(&query, 1, 32, 512, false, SQL_TIMEOUT)?;
        assert!(serde_json::to_vec(&bounded)?.len() <= 512, "{bounded}");
        assert_eq!(bounded["truncated"], true);
        assert!(
            bounded["truncation_reasons"]
                .as_array()
                .unwrap()
                .contains(&json!("header_byte_limit"))
        );
        assert_eq!(bounded["truncated_headers"], 2);
        let columns = bounded["columns"].as_array().unwrap();
        assert_eq!(columns.len(), 2);
        assert_ne!(columns[0], columns[1]);
        for (index, column) in columns.iter().enumerate() {
            let name = column.as_str().unwrap();
            assert!(name.len() <= 32);
            assert_eq!(bounded["rows"][0][name], index + 1, "{bounded}");
        }
        let full = store.sql_bounded(
            &query,
            usize::MAX,
            usize::MAX,
            usize::MAX,
            false,
            SQL_TIMEOUT,
        )?;
        assert_eq!(full["columns"], json!(names));
        assert_eq!(full["truncated"], false);
        // The original failure used the output ceiling alone, with the default cell cap.
        let short = store.sql_bounded(&query, 1, 2000, 256, false, SQL_TIMEOUT)?;
        assert!(serde_json::to_vec(&short)?.len() <= 256, "{short}");
        assert_eq!(short["truncated"], true);
        assert!(short["truncated_headers"].as_u64().unwrap() > 0);
    }
    Ok(())
}

#[test]
fn sql_output_budgets_include_escaped_headers_rows_and_truncation_metadata() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("history.duckdb");
    drop(Store::open(&db, false)?);
    let store = Store::open(&db, true)?;
    for budget in [0, 64, 128, 256, 512, 1024] {
        for (limit, cell_bytes) in [(0, 2000), (1, 16), (10, 2000)] {
            let result = store.sql_bounded(
                "SELECT repeat('界',100) AS \"long header with spaces\",repeat(chr(10),100) AS escaped FROM range(20)",
                limit, cell_bytes, budget, false, SQL_TIMEOUT,
            );
            match result {
                Ok(value) => assert!(
                    serde_json::to_vec(&value)?.len() <= budget,
                    "{budget}: {value}"
                ),
                Err(error) => assert!(
                    budget < 256 && error.to_string().contains("budget"),
                    "{budget}: {error}"
                ),
            }
        }
    }
    Ok(())
}

#[test]
fn sql_deadline_interrupts_engine_work_and_leaves_the_reader_usable() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("history.duckdb");
    drop(Store::open(&path, false)?);
    let store = Store::open(&path, true)?;
    let slow = store.sql_bounded(
        "SELECT sum(sin(i::DOUBLE)) AS total FROM range(10000000000) t(i)",
        1,
        2000,
        8000,
        false,
        std::time::Duration::from_millis(75),
    );
    assert!(
        slow.unwrap_err()
            .to_string()
            .contains("SQL query timed out"),
        "execution should fail with a deadline error"
    );
    let next = store.sql_bounded("SELECT 1 AS value", 1, 2000, 8000, false, SQL_TIMEOUT)?;
    assert_eq!(next["rows"], json!([{"value": 1}]));
    Ok(())
}

use crate::Store;
use anyhow::Result;
use llm_history_types::Harness;
use llm_history_types::query::Filter;

#[test]
fn usage_deduplicates_within_profiles_and_buckets_in_the_selected_timezone() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("usage.duckdb"), false)?;
    let fixture = include_str!("../../../../../fixtures/claude.jsonl");
    for name in ["personal", "work"] {
        let root = d.path().join(name);
        std::fs::create_dir_all(root.join("projects/p"))?;
        store.add_profile(name, Harness::Claude, &root, None, None)?;
        for file in ["original.jsonl", "copied.jsonl"] {
            std::fs::write(root.join("projects/p").join(file), fixture)?;
        }
    }
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(receipt.errors.is_empty(), "{receipt:?}");
    let rows = store.usage(&Filter::default(), "America/Denver", false, true, 100)?;
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row.period, "2026-07-31");
        assert_eq!(row.input, 255);
        assert_eq!(row.output, 17);
        assert_eq!(row.cached, 130);
        assert_eq!(row.cache_write, 120);
        assert_eq!(row.thinking, 5);
        assert_eq!(row.model.as_deref(), Some("claude-opus-test"));
        assert_eq!(row.effort.as_deref(), Some("high"));
    }
    let weekly = store.usage(
        &Filter {
            profiles: vec!["work".into()],
            ..Filter::default()
        },
        "UTC",
        true,
        false,
        100,
    )?;
    assert_eq!(weekly[0].period, "2026-07-27");
    assert_eq!(weekly.len(), 1);
    // The copied occurrence loses deduplication before date filtering, even
    // when it alone falls inside the requested time range.
    store.database.connection().execute_batch(
        "UPDATE events SET timestamp_utc=timestamp_utc+INTERVAL 1 DAY
         WHERE source_id IN (SELECT id FROM sources WHERE uri LIKE '%/copied.jsonl')",
    )?;
    let selected = Filter {
        profiles: vec!["work".into()],
        source: Some("claude".into()),
        since: Some("2026-08-01T00:00:01Z".into()),
        until: Some("2026-08-01T00:00:01Z".into()),
        ..Filter::default()
    };
    let one = store.usage(&selected, "UTC", false, false, 1)?;
    assert_eq!(one.len(), 1);
    assert_eq!((one[0].profile.as_str(), one[0].output), ("work", 10));
    let later = Filter {
        since: Some("2026-08-02T00:00:00Z".into()),
        until: None,
        ..selected
    };
    assert!(store.usage(&later, "UTC", false, false, 1)?.is_empty());
    Ok(())
}

#[test]
fn absent_imported_cache_counts_remain_zero() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("usage.duckdb"), false)?;
    let root = d.path().join("claude");
    std::fs::create_dir_all(root.join("projects/p"))?;
    store.add_profile("personal", Harness::Claude, &root, None, None)?;
    std::fs::write(
        root.join("projects/p/session.jsonl"),
        include_str!("../../../../../fixtures/claude.jsonl"),
    )?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    store.database.connection().execute_batch(
        "update provider_token_events \
         set last_cached_input_tokens=NULL,\
         last_cache_write_input_tokens=NULL,\
         last_reasoning_output_tokens=NULL",
    )?;
    let usage = store.usage(&Filter::default(), "UTC", false, false, 10)?;
    assert_eq!(usage[0].input, 255);
    assert_eq!(usage[0].output, 17);
    assert_eq!(usage[0].cached, 0);
    assert_eq!(usage[0].cache_write, 0);
    assert_eq!(usage[0].thinking, 0);
    Ok(())
}

#[test]
fn appended_blocks_of_a_counted_message_are_not_counted_again() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("usage.duckdb"), false)?;
    let root = d.path().join("claude");
    std::fs::create_dir_all(root.join("projects/p"))?;
    store.add_profile("personal", Harness::Claude, &root, None, None)?;
    let block = |uuid: &str, id: &str, output: u64| {
        format!(
            "{}\n",
            serde_json::json!({"sessionId":"s","type":"assistant","uuid":uuid,"timestamp":"2026-08-01T00:00:01Z","message":{"id":id,"role":"assistant","model":"m","content":[{"type":"text","text":uuid}],"usage":{"input_tokens":1,"output_tokens":output}}})
        )
    };
    let path = root.join("projects/p/s.jsonl");
    std::fs::write(&path, block("b1", "message-1", 10))?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    // A later block of the same API message repeats its usage; a new message adds its own.
    let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
    std::io::Write::write_all(
        &mut file,
        format!(
            "{}{}",
            block("b2", "message-1", 10),
            block("b3", "message-2", 7)
        )
        .as_bytes(),
    )?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!(receipt.appended, 1, "{receipt:?}");
    let rows = store.usage(&Filter::default(), "UTC", false, false, 100)?;
    assert_eq!(rows[0].output, 17);
    let state: String =
        store
            .database
            .connection()
            .query_row("SELECT parser_state FROM sources", [], |r| r.get(0))?;
    assert!(!state.contains("message-1"), "{state}");
    Ok(())
}

use crate::Store;
use anyhow::Result;
use llm_history_types::Harness;
use std::io::Write;

type Facts = Vec<(
    u64,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
    String,
)>;
fn facts(store: &Store) -> Result<Facts> {
    Ok(store
        .database
        .connection()
        .prepare(
            "select ordinal,kind,role,phase,text,input_tokens,output_tokens,\
             cache_read,cache_write,usage_key \
             from events \
             where active \
             order by ordinal,fact_index",
        )?
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

#[test]
fn full_and_incremental_claude_capture_converge_including_deduplicated_usage() -> Result<()> {
    let d = tempfile::tempdir()?;
    let fixture = include_str!("../../../../../fixtures/claude.jsonl");
    let mut normalized = Vec::new();
    for incremental in [true, false] {
        let name = if incremental { "incremental" } else { "full" };
        let root = d.path().join(name);
        std::fs::create_dir_all(root.join("projects/p"))?;
        let path = root.join("projects/p/session.jsonl");
        let store = Store::open(&d.path().join(format!("{name}.duckdb")), false)?;
        store.add_profile(name, Harness::Claude, &root, None, None)?;
        if incremental {
            let lines = fixture.split_inclusive('\n').collect::<Vec<_>>();
            std::fs::write(&path, lines[..5].concat())?;
            assert!(
                crate::tests::fixtures::capture(&store, &[], false)?
                    .errors
                    .is_empty()
            );
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)?
                .write_all(lines[5..].concat().as_bytes())?;
        } else {
            std::fs::write(&path, fixture)?;
        }
        assert!(
            crate::tests::fixtures::capture(&store, &[], false)?
                .errors
                .is_empty()
        );
        normalized.push(facts(&store)?);
        store.verify()?;
        let source = store
            .database
            .connection()
            .query_row("select id from sources", [], |r| r.get(0))?;
        let mut archived = Vec::new();
        store.export_raw(source, None, &mut archived)?;
        assert_eq!(archived, fixture.as_bytes());
    }
    assert_eq!(normalized[0], normalized[1]);
    Ok(())
}

#[test]
fn source_failure_rolls_back_native_archive_events_and_checkpoint_together() -> Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let path = root.join("sessions/session.jsonl");
    let fixture = include_bytes!("../../../../../fixtures/codex.jsonl");
    std::fs::write(&path, fixture)?;
    let store = Store::open(&d.path().join("rollback.duckdb"), false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let before = facts(&store)?;
    let archive_counts = |store: &Store| {
        store.database.connection().query_row(
            "SELECT (SELECT count(*) FROM content_objects), (SELECT count(*) FROM content_parts),
                    (SELECT count(*) FROM native_observations), (SELECT count(*) FROM captures)",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
    };
    let before_archive = archive_counts(&store)?;
    let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
    writeln!(
        file,
        "{}",
        serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":"This content object must roll back"}})
    )?;
    file.flush()?;
    // Fail the event write after publishing the archive within the transaction.
    store
        .database
        .connection()
        .execute_batch("ALTER TABLE temp.stage_events ALTER COLUMN text TYPE INTEGER")?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!(result.errors.len(), 1);
    assert_eq!(facts(&store)?, before);
    assert_eq!(archive_counts(&store)?, before_archive);
    assert_eq!(store.verify()?["generations"], 1);
    let source = store
        .database
        .connection()
        .query_row("select id from sources", [], |r| r.get(0))?;
    let mut archived = Vec::new();
    store.export_raw(source, None, &mut archived)?;
    assert_eq!(archived, fixture);
    Ok(())
}

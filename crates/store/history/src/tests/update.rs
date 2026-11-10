use crate::Store;
use anyhow::Result;
use llm_history_core::fixture_record;
use llm_history_types::Harness;
use llm_history_types::query::Filter;
use std::fs;

#[cfg(unix)]
#[test]
fn an_unreadable_file_does_not_block_the_rest_of_its_batch() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    fs::create_dir_all(root.join("sessions"))?;
    for name in ["a", "b", "c"] {
        let records = fixture_record(Harness::Codex, name, &format!("message {name}"));
        fs::write(
            root.join(format!("sessions/{name}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    let locked = root.join("sessions/b.jsonl");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o600))?;
    assert_eq!(receipt.imported, 2, "{receipt:?}");
    assert_eq!(receipt.errors.len(), 1, "{receipt:?}");
    let mut texts = store
        .messages_page(&Filter::default(), None, usize::MAX)?
        .into_iter()
        .map(|m| m.text)
        .collect::<Vec<_>>();
    texts.sort();
    assert_eq!(texts, ["message a", "message c"]);
    assert_eq!(
        crate::tests::fixtures::capture(&store, &[], false)?.imported,
        1
    );
    assert_eq!(
        store
            .messages_page(&Filter::default(), None, usize::MAX)?
            .len(),
        3
    );
    Ok(())
}

#[test]
fn large_append_only_tables_have_no_hash_keyed_indexes() -> Result<()> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("history.duckdb");
    let store = Store::open(&db, false)?;
    let keys: Vec<(String, String)> = store
        .database
        .connection()
        .prepare(
            "SELECT table_name,constraint_type \
             FROM duckdb_constraints() \
             WHERE table_name IN ('native_documents','native_observations','content_objects','content_parts','events') \
             AND constraint_type IN ('PRIMARY KEY','UNIQUE') \
             ORDER BY 1",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<duckdb::Result<_>>()?;
    assert_eq!(keys, [("events".to_string(), "PRIMARY KEY".to_string())]);
    let indexes: i64 = store.database.connection().query_row(
        "SELECT count(*) FROM duckdb_indexes() WHERE table_name IN ('native_documents','native_observations','content_objects','content_parts','events')",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(indexes, 0);
    Ok(())
}

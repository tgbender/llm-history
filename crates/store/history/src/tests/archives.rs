use crate::Store;
use crate::archives::restore_archive;
use crate::archives::verify_archive;
use anyhow::Result;
use llm_history_core::fixture_record;
use llm_history_types::Harness;
use llm_history_types::query::Filter;
use llm_history_types::storage::DatabaseBusy;
use std::path::Path;

fn write_history(root: &Path, text: &str) -> Result<()> {
    std::fs::create_dir_all(root.join("sessions"))?;
    std::fs::write(
        root.join("sessions/session.jsonl"),
        fixture_record(Harness::Codex, "thread", text)
            .iter()
            .map(|r| format!("{r}\n"))
            .collect::<String>(),
    )?;
    Ok(())
}
#[test]
fn restore_requires_permission_backs_up_active_data_and_keeps_a_stable_lock() -> Result<()> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("active.duckdb");
    let archive = d.path().join("archive.duckdb");
    let root = d.path().join("codex");
    write_history(&root, "old")?;
    let store = Store::open(&db, false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    let snapshot = store.snapshot(&archive)?;
    assert_eq!(verify_archive(&archive)?.bytes, snapshot.bytes);
    write_history(&root, "new")?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    // Recovery must preserve a damaged current store without requiring it to verify.
    store
        .database
        .connection()
        .execute_batch("update native_documents set xxh64=xor(xxh64,1)")?;
    assert!(
        restore_archive(&archive, &db, true)
            .unwrap_err()
            .downcast_ref::<DatabaseBusy>()
            .is_some()
    );
    drop(store);
    assert!(restore_archive(&archive, &db, false).is_err());
    let reader = Store::open(&db, true)?;
    assert!(
        restore_archive(&archive, &db, true)
            .unwrap_err()
            .downcast_ref::<DatabaseBusy>()
            .is_some()
    );
    drop(reader);
    let result = restore_archive(&archive, &db, true)?;
    let backup = result.archived_previous.unwrap();
    assert!(verify_archive(&backup).is_err());
    let old = Store::open(&db, true)?;
    assert_eq!(
        old.messages_page(&Filter::default(), None, usize::MAX)?[0].text,
        "old"
    );
    assert!(Store::open(&db, false).is_err());
    drop(old);
    let previous = Store::open(&backup, true)?;
    assert_eq!(
        previous.messages_page(&Filter::default(), None, usize::MAX)?[0].text,
        "new"
    );
    assert!(!d.path().read_dir()?.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with(".history-publish-")
    }));
    Ok(())
}
#[test]
fn corrupt_archive_leaves_active_database_and_absent_destination_untouched() -> Result<()> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("active.duckdb");
    let archive = d.path().join("bad.duckdb");
    let root = d.path().join("codex");
    write_history(&root, "preserve")?;
    let store = Store::open(&db, false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    store.snapshot(&archive)?;
    drop(store);
    std::fs::write(&archive, b"not a DuckDB database")?;
    let before = std::fs::read(&db)?;
    assert!(restore_archive(&archive, &db, true).is_err());
    assert_eq!(std::fs::read(&db)?, before);
    let absent = d.path().join("absent.duckdb");
    assert!(restore_archive(&archive, &absent, false).is_err());
    assert!(!absent.exists());
    assert!(!d.path().read_dir()?.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with(".history-publish-")
    }));
    Ok(())
}
#[test]
fn snapshot_refuses_existing_targets_and_preserves_their_bytes() -> Result<()> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("active.duckdb");
    let archive = d.path().join("existing.duckdb");
    let store = Store::open(&db, false)?;
    std::fs::write(&archive, b"existing")?;
    assert!(store.snapshot(&archive).is_err());
    assert_eq!(std::fs::read(archive)?, b"existing");
    Ok(())
}

#[test]
fn archives_require_the_current_schema() -> Result<()> {
    let d = tempfile::tempdir()?;
    let archive = d.path().join("archive.duckdb");
    let root = d.path().join("codex");
    write_history(&root, "old")?;
    let store = Store::open(&archive, false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    drop(store);
    for (version, accepted) in [("1", true), ("13", false)] {
        duckdb::Connection::open(&archive)?.execute(
            "UPDATE meta SET value=? WHERE key='schema_version'",
            [version],
        )?;
        let result = verify_archive(&archive);
        assert_eq!(result.is_ok(), accepted, "{version}: {result:?}");
    }
    Ok(())
}

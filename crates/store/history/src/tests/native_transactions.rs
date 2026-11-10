//! Shared history and native repositories publish through one transaction.
use crate::Store;
use anyhow::Result;
use llm_history_store_engine::WriteTransaction;
use llm_history_store_harnesses::{catalog, codex::threads};
use llm_history_types::{Harness, native::codex::ThreadMetadata};
use std::collections::BTreeSet;

fn stage(transaction: &WriteTransaction<'_>, profile: &str) -> Result<()> {
    let connection = transaction.connection();
    connection.execute(
        "INSERT INTO sessions(id,profile_id,native_id) VALUES(1,?,'thread')",
        [profile],
    )?;
    connection.execute(
        "INSERT INTO sources(id,profile_id,source_key,committed) VALUES(1,?,'source',42)",
        [profile],
    )?;
    threads::write(
        transaction,
        profile,
        &[ThreadMetadata {
            id: "thread".into(),
            name: "native title".into(),
            model: "model".into(),
            ..Default::default()
        }],
    )?;
    catalog::write_snapshot(
        transaction,
        catalog::SnapshotWrite {
            id: 1,
            profile,
            uri: "synthetic://catalog",
            format: "fixture",
            digest: "fixture",
            data: "{}",
        },
    )?;
    catalog::reconcile_assignments(
        transaction,
        profile,
        &BTreeSet::from([("thread".into(), "project".into())]),
    )?;
    Ok(())
}

fn setup() -> Result<(tempfile::TempDir, Store, String)> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(&directory.path().join("history.duckdb"), false)?;
    let profile = store.add_profile("fixture", Harness::Codex, directory.path(), None, None)?;
    Ok((directory, store, profile.id))
}

#[test]
fn a_late_failure_rolls_back_shared_native_and_checkpoint_writes() -> Result<()> {
    let (_directory, store, profile) = setup()?;
    {
        let transaction = store.database.begin()?;
        stage(&transaction, &profile)?;
        assert!(
            transaction
                .connection()
                .execute("INSERT INTO sources(id) VALUES(1)", [])
                .is_err()
        );
    }
    for table in [
        "sessions",
        "sources",
        "native_threads",
        "catalog_snapshots",
        "native_project_assignments",
    ] {
        let count: i64 = store.database.connection().query_row(
            &format!("SELECT count(*) FROM {table}"),
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0, "{table} escaped rollback");
    }
    // A failed transaction releases its state so the next import can proceed.
    let transaction = store.database.begin()?;
    stage(&transaction, &profile)?;
    transaction.commit()?;
    assert_eq!(
        store
            .database
            .connection()
            .query_row("SELECT committed FROM sources", [], |row| row
                .get::<_, u64>(0))?,
        42
    );
    Ok(())
}

#[test]
fn shared_native_and_checkpoint_changes_commit_and_reopen_together() -> Result<()> {
    let (directory, store, profile) = setup()?;
    let transaction = store.database.begin()?;
    stage(&transaction, &profile)?;
    transaction.commit()?;
    drop(store);
    let store = Store::open(&directory.path().join("history.duckdb"), true)?;
    let values:(u64,String,String)=store.database.connection().query_row(
        "SELECT s.committed,t.name,a.project_native FROM sources s JOIN native_threads t ON t.profile_id=s.profile_id JOIN native_project_assignments a ON a.profile_id=t.profile_id AND a.session_native=t.id",
        [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    )?;
    assert_eq!(values, (42, "native title".into(), "project".into()));
    assert!(catalog::current(store.database.connection(), &profile)?.is_some());
    Ok(())
}

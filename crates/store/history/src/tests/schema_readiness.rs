use crate::{SCHEMA_VERSION, Store};
use anyhow::Result;

fn meta(path: &std::path::Path, key: &str) -> Result<Option<String>> {
    use duckdb::OptionalExt;
    let con = duckdb::Connection::open(path)?;
    Ok(con
        .query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))
        .optional()?)
}

/// A database as version 1 left it: no definitions digest, and a view this build replaced.
fn version_one(path: &std::path::Path) -> Result<()> {
    drop(Store::open(path, false)?);
    let con = duckdb::Connection::open(path)?;
    con.execute_batch(
        "UPDATE meta SET value='1' WHERE key='schema_version';
         DELETE FROM meta WHERE key='definitions';
         CREATE OR REPLACE VIEW history_help AS SELECT 'stale' AS surface;",
    )?;
    Ok(())
}

fn help_is_current(store: &Store) -> Result<bool> {
    Ok(store
        .database
        .connection()
        .query_row("SELECT count(*) > 1 FROM history_help", [], |r| r.get(0))?)
}

#[test]
fn a_writer_upgrades_an_older_database_and_its_views() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("history.duckdb");
    version_one(&path)?;
    let identity = meta(&path, "identity")?;
    let store = Store::open(&path, false)?;
    assert!(help_is_current(&store)?);
    drop(store);
    assert_eq!(
        meta(&path, "schema_version")?,
        Some(SCHEMA_VERSION.to_string())
    );
    assert!(meta(&path, "definitions")?.is_some());
    assert_eq!(meta(&path, "identity")?, identity);
    Ok(())
}

#[test]
fn a_reader_has_an_older_database_upgraded_first() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("history.duckdb");
    version_one(&path)?;
    let store = Store::open(&path, true)?;
    assert!(help_is_current(&store)?);
    drop(store);
    assert_eq!(
        meta(&path, "schema_version")?,
        Some(SCHEMA_VERSION.to_string())
    );
    Ok(())
}

#[test]
fn views_from_another_build_are_replaced_without_a_version_change() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("history.duckdb");
    drop(Store::open(&path, false)?);
    let recorded = meta(&path, "definitions")?;
    {
        let con = duckdb::Connection::open(&path)?;
        con.execute_batch(
            "UPDATE meta SET value='another build' WHERE key='definitions';
             CREATE OR REPLACE VIEW history_help AS SELECT 'stale' AS surface;",
        )?;
    }
    let store = Store::open(&path, true)?;
    assert!(help_is_current(&store)?);
    drop(store);
    assert_eq!(meta(&path, "definitions")?, recorded);
    Ok(())
}

#[test]
fn a_database_from_a_newer_build_is_refused_and_left_alone() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("history.duckdb");
    drop(Store::open(&path, false)?);
    let newer = (SCHEMA_VERSION + 1).to_string();
    duckdb::Connection::open(&path)?.execute(
        "UPDATE meta SET value=? WHERE key='schema_version'",
        [&newer],
    )?;
    for read_only in [false, true] {
        let Err(error) = Store::open(&path, read_only) else {
            panic!("a newer schema was opened")
        };
        assert!(
            error.to_string().contains("newer than this build"),
            "{error}"
        );
    }
    assert_eq!(meta(&path, "schema_version")?, Some(newer));
    Ok(())
}

/// Every table and view column, macro, sequence and index: what a schema amounts to.
fn catalog(path: &std::path::Path) -> Result<Vec<String>> {
    let con = duckdb::Connection::open(path)?;
    let mut entries = Vec::new();
    for query in [
        "SELECT table_name || '.' || column_name || ' ' || data_type \
         FROM information_schema.columns WHERE table_schema='main' ORDER BY 1",
        "SELECT 'macro ' || function_name || '(' || array_to_string(parameters, ',') || ')' \
         FROM duckdb_functions() WHERE NOT internal AND schema_name='main' ORDER BY 1",
        "SELECT 'sequence ' || sequence_name FROM duckdb_sequences() ORDER BY 1",
        "SELECT 'index ' || index_name FROM duckdb_indexes() ORDER BY 1",
    ] {
        let mut statement = con.prepare(query)?;
        let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
        entries.extend(rows.collect::<duckdb::Result<Vec<_>>>()?);
    }
    Ok(entries)
}

/// The check that keeps migrations honest. `fixtures/schema/1.sql` is the layout 0.1.0
/// shipped and never changes. Whatever the schema becomes, upgrading that database has
/// to arrive at exactly what a new database gets.
#[test]
fn the_first_release_upgrades_to_the_schema_of_a_new_database() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (released, new) = (
        directory.path().join("released.duckdb"),
        directory.path().join("new.duckdb"),
    );
    {
        let con = duckdb::Connection::open(&released)?;
        con.execute_batch(include_str!("../../../../../fixtures/schema/1.sql"))?;
        con.execute_batch(
            "INSERT INTO meta VALUES ('schema_version','1'),('identity','released');
             INSERT INTO profiles VALUES ('p','kept','{}');",
        )?;
    }
    drop(Store::open(&released, false)?);
    drop(Store::open(&new, false)?);
    let upgraded = catalog(&released)?;
    assert!(upgraded.len() > 300, "{}", upgraded.len());
    assert_eq!(upgraded, catalog(&new)?);
    assert_eq!(
        meta(&released, "schema_version")?,
        Some(SCHEMA_VERSION.to_string())
    );
    assert_eq!(meta(&released, "identity")?, Some("released".into()));
    let kept: String =
        duckdb::Connection::open(&released)?
            .query_row("SELECT name FROM profiles", [], |r| r.get(0))?;
    assert_eq!(kept, "kept");
    Ok(())
}

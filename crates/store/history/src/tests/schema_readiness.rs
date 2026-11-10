use crate::Store;
use anyhow::Result;

#[test]
fn an_old_development_layout_requires_manual_conversion() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("history.duckdb");
    let store = Store::open(&path, false)?;
    store
        .database
        .connection()
        .execute("UPDATE meta SET value='13' WHERE key='schema_version'", [])?;
    drop(store);
    let Err(error) = Store::open(&path, false) else {
        panic!("old development schema was opened")
    };
    assert!(error.to_string().contains("convert a copy manually"));
    let con = duckdb::Connection::open(&path)?;
    let version: String = con.query_row(
        "SELECT value FROM meta WHERE key='schema_version'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(version, "13");
    Ok(())
}

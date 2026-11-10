//! Update decides unchanged/append/rewrite from metadata and two small windows,
//! without re-reading whole transcripts.
use crate::Store;
use anyhow::Result;
use llm_history_core::fixture_record;
use llm_history_types::Harness;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

fn line(text: &str) -> String {
    fixture_record(Harness::Codex, "s", text)
        .iter()
        .map(|v| format!("{v}\n"))
        .collect()
}
fn texts(store: &Store) -> Result<Vec<String>> {
    Ok(store
        .database
        .connection()
        .prepare("select text from events where active and kind='message' order by ordinal")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}
fn setup(d: &Path, body: &str) -> Result<(Store, std::path::PathBuf)> {
    let root = d.join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let path = root.join("sessions/a.jsonl");
    std::fs::write(&path, body)?;
    let store = Store::open(&d.join("h.duckdb"), false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    Ok((store, path))
}
fn rewrite_keeping_metadata(path: &Path, body: &str) -> Result<()> {
    let modified = path.metadata()?.modified()?;
    std::fs::write(path, body)?;
    File::options()
        .write(true)
        .open(path)?
        .set_modified(modified)?;
    Ok(())
}

#[test]
fn unchanged_size_and_mtime_skip_reading_the_source() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, path) = setup(d.path(), &line("alpha"))?;
    // Same length and mtime: the file is not opened, so the edit is not seen.
    rewrite_keeping_metadata(&path, &line("bravo"))?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!((receipt.unchanged, receipt.records), (1, 0));
    assert_eq!(texts(&store)?, ["alpha"]);
    Ok(())
}

#[test]
fn appends_resume_from_the_checkpoint() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, path) = setup(d.path(), &line("alpha"))?;
    OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(line("bravo").as_bytes())?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!(receipt.appended, 1);
    assert_eq!(texts(&store)?, ["alpha", "bravo"]);
    store.verify()?;
    Ok(())
}

#[test]
fn appends_extend_the_generation_without_copying_records() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, path) = setup(d.path(), &line("alpha"))?;
    for text in ["bravo", "charlie", "delta"] {
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(line(text).as_bytes())?;
        assert_eq!(
            crate::tests::fixtures::capture(&store, &[], false)?.appended,
            1
        );
    }
    let count = |sql: &str| -> Result<i64> {
        Ok(store
            .database
            .connection()
            .query_row(sql, [], |r| r.get(0))?)
    };
    let lines = std::fs::read(&path)?
        .iter()
        .filter(|b| **b == b'\n')
        .count() as i64;
    assert_eq!(count("select count(*) from generations")?, 1);
    assert_eq!(count("select count(*) from native_observations")?, lines);
    assert_eq!(count("select lines from generations")?, lines);
    assert_eq!(texts(&store)?, ["alpha", "bravo", "charlie", "delta"]);
    let source = count("select id from sources")?;
    let mut raw = Vec::new();
    store.export_raw(source, None, &mut raw)?;
    assert_eq!(raw, std::fs::read(&path)?);
    store.verify()?;
    Ok(())
}

#[test]
fn edits_inside_either_window_are_rewrites() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, path) = setup(d.path(), &line("alpha"))?;
    // Same length, new mtime and a changed committed prefix, plus an append.
    std::fs::write(&path, line("bravo") + &line("delta"))?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!(receipt.imported, 1);
    assert_eq!(texts(&store)?, ["bravo", "delta"]);
    let generations: i64 =
        store
            .database
            .connection()
            .query_row("select count(*) from generations", [], |r| r.get(0))?;
    assert_eq!(generations, 2);
    store.verify()?;
    Ok(())
}

#[test]
fn stores_without_windows_are_adopted_by_their_full_prefix_hash() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, path) = setup(d.path(), &line("alpha"))?;
    let committed: u64 =
        store
            .database
            .connection()
            .query_row("select committed from sources", [], |r| r.get(0))?;
    let prefix = llm_history_core::hash_range(&path, 0, committed)?;
    store.database.connection().execute(
        "update sources \
         set prefix_hash=?,head_xxh64=null,boundary_xxh64=null,\
         observed_size=null,observed_mtime_ns=null",
        [prefix],
    )?;
    OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(line("bravo").as_bytes())?;
    assert_eq!(
        crate::tests::fixtures::capture(&store, &[], false)?.appended,
        1
    );
    let windows: (Option<u64>, Option<u64>) = store.database.connection().query_row(
        "select head_xxh64,boundary_xxh64 from sources",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert!(windows.0.is_some() && windows.1.is_some());
    assert_eq!(texts(&store)?, ["alpha", "bravo"]);
    Ok(())
}

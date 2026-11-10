//! Codex compresses finished rollouts to `.jsonl.zst`. A compressed file is the same
//! source as its `.jsonl`, so compression continues a session instead of duplicating it.
use crate::Store;
use anyhow::Result;
use llm_history_core::fixture_record;
use llm_history_types::Harness;
use std::path::{Path, PathBuf};

fn line(text: &str) -> String {
    fixture_record(Harness::Codex, "s", text)
        .iter()
        .map(|v| format!("{v}\n"))
        .collect()
}
fn compress(body: &str) -> Result<Vec<u8>> {
    Ok(zstd::encode_all(body.as_bytes(), 3)?)
}
fn texts(store: &Store) -> Result<Vec<String>> {
    Ok(store
        .database
        .connection()
        .prepare("select text from events where active and kind='message' order by ordinal")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}
fn sources(store: &Store) -> Result<Vec<(String, String, bool)>> {
    Ok(store
        .database
        .connection()
        .prepare("select source_key,uri,missing from sources order by id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?)
}
fn setup(d: &Path) -> Result<(Store, PathBuf)> {
    let root = d.join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let store = Store::open(&d.join("h.duckdb"), false)?;
    store.add_profile("personal", Harness::Codex, &root, None, None)?;
    Ok((store, root.join("sessions")))
}

#[test]
fn compressed_sessions_are_captured() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, dir) = setup(d.path())?;
    let body = line("alpha") + &line("bravo");
    std::fs::write(dir.join("a.jsonl.zst"), compress(&body)?)?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
    assert_eq!(receipt.imported, 1);
    assert_eq!(texts(&store)?, ["alpha", "bravo"]);
    let (key, uri, missing) = sources(&store)?.remove(0);
    assert_eq!(key, Path::new("sessions/a.jsonl").to_string_lossy());
    assert!(uri.ends_with("a.jsonl.zst") && !missing);
    let id: i64 = store
        .database
        .connection()
        .query_row("select id from sources", [], |r| r.get(0))?;
    let mut raw = Vec::new();
    store.export_raw(id, None, &mut raw)?;
    assert_eq!(raw, body.as_bytes());
    store.verify()?;
    // Unchanged compressed files are skipped like plain ones.
    assert_eq!(
        crate::tests::fixtures::capture(&store, &[], false)?.unchanged,
        1
    );
    Ok(())
}

#[test]
fn compressing_a_captured_session_continues_the_same_source() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, dir) = setup(d.path())?;
    std::fs::write(dir.join("a.jsonl"), line("alpha"))?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let ids: Vec<i64> = store
        .database
        .connection()
        .prepare("select id from events where active order by id")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    // The session finished with one more line, then was compressed in place.
    std::fs::write(
        dir.join("a.jsonl.zst"),
        compress(&(line("alpha") + &line("bravo")))?,
    )?;
    std::fs::remove_file(dir.join("a.jsonl"))?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
    assert_eq!((receipt.appended, receipt.missing), (1, 0));
    assert_eq!(texts(&store)?, ["alpha", "bravo"]);
    assert_eq!(sources(&store)?.len(), 1);
    let kept: Vec<i64> = store
        .database
        .connection()
        .prepare("select id from events where active order by id")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    assert_eq!(kept[..ids.len()], ids[..]);
    store.verify()?;
    Ok(())
}

#[test]
fn a_live_jsonl_wins_over_a_compressed_copy_in_progress() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, dir) = setup(d.path())?;
    std::fs::write(dir.join("a.jsonl"), line("alpha"))?;
    let whole = compress(&line("alpha"))?;
    std::fs::write(dir.join("a.jsonl.zst"), &whole[..whole.len() / 2])?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
    assert_eq!(texts(&store)?, ["alpha"]);
    assert!(sources(&store)?[0].1.ends_with("a.jsonl"));
    Ok(())
}

#[test]
fn a_truncated_compressed_file_fails_and_is_retried() -> Result<()> {
    let d = tempfile::tempdir()?;
    let (store, dir) = setup(d.path())?;
    let whole = compress(&(line("alpha") + &line("bravo")))?;
    std::fs::write(dir.join("a.jsonl.zst"), &whole[..whole.len() - 4])?;
    assert_eq!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .len(),
        1
    );
    assert!(texts(&store)?.is_empty());
    std::fs::write(dir.join("a.jsonl.zst"), &whole)?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(receipt.errors.is_empty(), "{:?}", receipt.errors);
    assert_eq!(texts(&store)?, ["alpha", "bravo"]);
    Ok(())
}

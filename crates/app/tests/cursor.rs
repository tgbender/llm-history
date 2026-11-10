//! Public import and replay contracts against the synthetic native Cursor store.
use anyhow::Result;
use llm_history_app::{
    History, ProfileRegistration,
    api::{ingestion::UpdateRequest, query::MessageQuery, sql::SqlRequest},
};
use llm_history_types::{
    Harness,
    archive::RawFilter,
    query::{Filter, Message, Selection},
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

const SESSION: &str = "00000000-0000-4000-8000-000000000031";
const BUCKET: &str = "6c5951e6d4681ef9222bcf4714f116f2";
const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/native-history/cursor"
);
const REPLY: &str = "Amber succeeded; the input file was missing.";
const ABANDONED_ROOT: &str = "1d7840870e641b8a16359c48905a202fcb1f31165e0093e2d16087d7695d09c4";

fn copy(root: &Path, relative: &Path) -> Result<PathBuf> {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().unwrap())?;
    fs::copy(Path::new(CORPUS).join("home").join(relative), &target)?;
    Ok(target)
}

fn setup(dir: &Path) -> Result<(PathBuf, History)> {
    let root = dir.join("cursor");
    let transcript = copy(
        &root,
        &PathBuf::from(format!(
            "projects/fixture-cursor/agent-transcripts/{SESSION}/{SESSION}.jsonl"
        )),
    )?;
    let history = History::new(dir.join("history.duckdb"));
    history.register_profile(&ProfileRegistration {
        name: "cursor".into(),
        harness: Harness::Cursor,
        root,
        account: None,
        backend: None,
    })?;
    Ok((transcript, history))
}

fn sql(history: &History, query: &str) -> Result<Value> {
    Ok(history.sql(&SqlRequest {
        query: query.into(),
        ..Default::default()
    })?["rows"]
        .clone())
}

fn messages(history: &History) -> Result<Vec<Message>> {
    history.messages(&MessageQuery {
        filter: Filter {
            source: Some("cursor".into()),
            ..Default::default()
        },
        ..Default::default()
    })
}

#[test]
fn cursor_prefers_selected_store_and_keeps_good_archive_when_wal_snapshot_breaks() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_transcript, history) = setup(dir.path())?;
    let sidecar = copy(
        &dir.path().join("cursor"),
        &PathBuf::from(format!("chats/{BUCKET}/{SESSION}/meta.json")),
    )?;
    let store_path = sidecar.with_file_name("store.db");
    let native = Connection::open(&store_path)?;
    native.execute_batch("PRAGMA wal_autocheckpoint=0;")?;
    native.execute_batch(include_str!(
        "../../../fixtures/native-history/cursor/store.sql"
    ))?;
    native.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let before = store_path.metadata()?;
    let receipt = history.update(&UpdateRequest::default())?.receipt;
    assert!(receipt.errors.is_empty(), "{receipt:?}");
    assert_eq!(receipt.imported, 1);
    let selected = messages(&history)?;
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[1].text, REPLY);
    assert_eq!(
        sql(&history, "SELECT count(*) n FROM history_sessions")?,
        json!([{"n":1}])
    );
    assert_eq!(
        sql(
            &history,
            "SELECT native_call_id, output_preview FROM history_tool_activity ORDER BY native_call_id"
        )?,
        json!([
            {"native_call_id":"fixture-call-amber","output_preview":"amber"},
            {"native_call_id":"fixture-call-missing","output_preview":"missing.txt: No such file or directory"}
        ])
    );
    assert!(history.refresh(&[], Some("cursor"), false)?.is_empty());

    native.execute_batch(include_str!(
        "../../../fixtures/native-history/cursor/drop-active-result.sql"
    ))?;
    assert_eq!(before.len(), store_path.metadata()?.len());
    assert_eq!(before.modified()?, store_path.metadata()?.modified()?);
    let result = history.refresh(&[], Some("cursor"), false)?;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].errors.len(), 1);
    assert_eq!(messages(&history)?[1].message_no, selected[1].message_no);
    assert_eq!(messages(&history)?[1].text, REPLY);
    assert_eq!(
        history
            .reprocess(&Selection::default(), &[], false)?
            .sources,
        1
    );
    assert_eq!(messages(&history)?[1].message_no, selected[1].message_no);

    native.execute(
        "UPDATE meta SET value=lower(hex(json_set(CAST(unhex(value) AS TEXT),'$.latestRootBlobId',?))) WHERE key='0'",
        [ABANDONED_ROOT],
    )?;
    let receipt = history.update(&UpdateRequest::default())?.receipt;
    assert!(receipt.errors.is_empty(), "{receipt:?}");
    assert_eq!(
        messages(&history)?[1].text,
        "Abandoned reply: both checks succeeded."
    );
    let latest = RawFilter {
        latest: true,
        ..Default::default()
    };
    assert_eq!(history.verify_raw(&latest, false, false)?["verified"], true);
    assert_eq!(history.verify_raw(&latest, true, false)?["verified"], false);
    Ok(())
}

#[test]
fn cursor_transcript_appends_and_exports_original_bytes() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (transcript, history) = setup(dir.path())?;
    let first = history.update(&UpdateRequest::default())?.receipt;
    assert!(first.errors.is_empty(), "{first:?}");
    let before = messages(&history)?;
    assert_eq!(before.len(), 2);
    let mut raw = fs::read(&transcript)?;
    raw.extend_from_slice(
        br#"{"role":"user","message":{"content":[{"type":"text","text":"Repeat the check."}]}}"#,
    );
    raw.push(b'\n');
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&transcript)?;
    file.write_all(&raw[fs::metadata(&transcript)?.len() as usize..])?;
    drop(file);
    let second = history.update(&UpdateRequest::default())?.receipt;
    assert!(second.errors.is_empty(), "{second:?}");
    assert_eq!(second.appended, 1);
    let after = messages(&history)?;
    assert_eq!(after.len(), 3);
    assert_eq!(after[1].message_no, before[1].message_no);
    assert_eq!(after[2].text, "Repeat the check.");
    let sources = history.raw_sources(&RawFilter::default())?;
    assert_eq!(sources.len(), 1);
    let mut output = Vec::new();
    history.export_raw(sources[0].id, None, &mut output)?;
    assert_eq!(output, raw);
    Ok(())
}

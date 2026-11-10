use crate::Store;
use anyhow::Result;
use llm_history_types::{Harness, archive::RawFilter, query::Filter, storage::StoreOptions};
use std::fs;

const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/native-history"
);
const BODY: &str = "Fixture greeting: café 🦀\n";
const PARENT: &str = "rollout-2026-11-07T19-00-00-00000000-0000-4000-8000-000000000001.jsonl.zst";
const CHILD: &str = "rollout-2026-11-07T19-00-09-00000000-0000-4000-8000-000000000002.jsonl";

#[test]
fn native_documents_share_bodies_and_replay_byte_for_byte_without_sources() -> Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    let sessions = root.join("sessions");
    let archived = root.join("archived_sessions");
    fs::create_dir_all(&sessions)?;
    fs::create_dir_all(&archived)?;
    let parent = fs::read(format!("{CORPUS}/codex/home/archived_sessions/{PARENT}"))?;
    fs::write(archived.join(PARENT), &parent)?;
    let child = fs::read(format!("{CORPUS}/codex/home/sessions/2026/11/07/{CHILD}"))?;
    fs::write(sessions.join(CHILD), &child)?;
    let unsafe_records = fs::read(format!("{CORPUS}/recovery/unsafe.jsonl"))?;
    fs::write(sessions.join("recovery.jsonl"), &unsafe_records)?;

    let path = d.path().join("history.duckdb");
    let options = StoreOptions {
        batch_records: 2,
        ..Default::default()
    };
    let store = Store::open_with_options(&path, false, options)?;
    store.add_profile("fixture", Harness::Codex, &root, None, None)?;
    let update = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(update.errors.is_empty(), "{update:?}");
    let con = store.database.connection();
    let id = blake3::hash(BODY.as_bytes());
    let (count, data): (i64, Vec<u8>) = con.query_row(
        "SELECT count(*),any_value(data) FROM content_objects WHERE id=?",
        [id.as_bytes().as_slice()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(count, 1);
    assert_eq!(zstd::bulk::decompress(&data, BODY.len())?, BODY.as_bytes());
    let (references, occurrences): (i64, i64) = con.query_row(
        "SELECT count(*),count(DISTINCT r.raw_id) FROM content_parts p
         JOIN native_observations r ON r.payload_hash=p.document_hash
         WHERE p.content_id=?",
        [id.as_bytes().as_slice()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert!(references >= 5 && occurrences >= 4);
    let (size, layout): (usize, Vec<u8>) = con.query_row(
        "SELECT d.raw_size,d.data FROM native_documents d JOIN content_parts p ON p.document_hash=d.hash
         WHERE p.content_id=? AND p.native_path='/payload/content/0/text' LIMIT 1",
        [id.as_bytes().as_slice()], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let layout = zstd::bulk::decompress(&layout, size)?;
    assert!(layout.len() < size);
    let shape: serde_json::Value = serde_json::from_slice(&layout)?;
    assert!(shape["payload"]["content"][0]["text"].is_null());
    let before = store.messages_page(&Filter::default(), None, 100)?;
    assert!(!before.is_empty());
    let old_sources = store.raw_sources(&RawFilter::default())?;
    assert_eq!(old_sources.len(), 3);
    drop(store);
    fs::remove_dir_all(root)?;

    let store = Store::open(&path, false)?;
    let parent = zstd::stream::decode_all(parent.as_slice())?;
    let complete_end = unsafe_records.iter().rposition(|b| *b == b'\n').unwrap() + 1;
    for source in old_sources {
        let mut bytes = Vec::new();
        store.export_raw(source.id, Some(source.generation), &mut bytes)?;
        let expected = if source.path.ends_with(PARENT) {
            &parent
        } else if source.path.ends_with(CHILD) {
            &child
        } else {
            &unsafe_records[..complete_end]
        };
        assert_eq!(&bytes, expected);
    }
    assert_eq!(store.verify()?["verified"], true);
    let replay = crate::tests::fixtures::replay(&store, &[], None, &[], false)?;
    assert!(replay.errors.is_empty(), "{replay:?}");
    let after = store.messages_page(&Filter::default(), None, 100)?;
    assert_eq!(
        before.iter().map(|m| (&m.id, &m.text)).collect::<Vec<_>>(),
        after.iter().map(|m| (&m.id, &m.text)).collect::<Vec<_>>()
    );

    // A missing referenced object fails closed even though the document layout is valid JSON.
    store.database.connection().execute(
        "DELETE FROM content_objects WHERE id=?",
        [id.as_bytes().as_slice()],
    )?;
    assert!(store.verify().is_err());
    Ok(())
}

#[test]
fn cas_apply_inserts_only_bodies_referenced_by_new_documents() -> Result<()> {
    use crate::archive_data::ArchiveBatch;
    use llm_history_types::ingestion::ContentLeaf;

    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("history.duckdb"), false)?;
    let tx = store.database.begin()?;
    let con = tx.connection();
    let existing = b"{\"body\":\"unused\"}\n";
    let hash = blake3::hash(existing).to_hex().to_string();
    let compressed = zstd::bulk::compress(existing, 3)?;
    con.execute(
        "INSERT INTO native_documents VALUES (?,?,?,?)",
        duckdb::params![
            hash,
            existing.len() as u64,
            xxhash_rust::xxh64::xxh64(existing, 0),
            compressed,
        ],
    )?;

    let mut batch = ArchiveBatch::default();
    for (bytes, body) in [
        (existing.as_slice(), "unused"),
        (b"{\"body\":\"shared\",\"id\":1}\n".as_slice(), "shared"),
        (b"{\"body\":\"shared\",\"id\":2}\n".as_slice(), "shared"),
    ] {
        batch.prepare(
            bytes,
            vec![ContentLeaf {
                token: 8..(8 + body.len() + 2),
                native_path: "/body".into(),
                body: body.into(),
            }],
        )?;
    }
    batch.publish(con)?;
    let counts: (i64, i64, i64) = con.query_row(
        "SELECT (SELECT count(*) FROM native_documents),
                (SELECT count(*) FROM content_parts),
                (SELECT count(*) FROM content_objects)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(counts, (3, 2, 1));
    let old: Vec<u8> = con.query_row(
        "SELECT data FROM native_documents WHERE hash=?",
        [&hash],
        |r| r.get(0),
    )?;
    assert_eq!(old, compressed);
    tx.commit()?;
    assert_eq!(store.verify()?["verified"], true);
    Ok(())
}

use crate::Store;
use anyhow::Result;
use duckdb::arrow::{
    array::{ArrayRef, Int64Array, StringArray},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use std::sync::Arc;

#[test]
fn duckdb_release_arrow_incremental_fts_and_icu() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("native.duckdb"), false)?;
    assert_eq!(
        store.status(&[], None, llm_history_ingest::PARSER_VERSION)?["duckdb"],
        "v2.0.0"
    );
    store
        .database
        .connection()
        .execute_batch("create table arrow_smoke(id BIGINT,text VARCHAR)")?;
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("text", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2])) as ArrayRef,
            Arc::new(StringArray::from(vec!["alpha", "unicode 🦀"])),
        ],
    )?;
    {
        let mut appender = store.database.connection().appender("arrow_smoke")?;
        appender.append_record_batch(batch)?;
        appender.flush()?;
    }
    assert_eq!(
        store
            .database
            .connection()
            .query_row("select count(*) from arrow_smoke", [], |r| r
                .get::<_, i64>(0))?,
        2
    );
    store.create_fts()?;
    store.database.connection().execute_batch(
        "insert into lexical_documents values (1,1,'opaque Rust handle','opaque rust handle')",
    )?;
    assert_eq!(store.fts_scores("opaque")?.len(), 1);
    store.database.connection().execute_batch(
        "begin; insert into lexical_documents values (2,2,'second document','second'); rollback",
    )?;
    assert!(store.fts_scores("second")?.is_empty());
    store.database.connection().execute_batch(
        "delete from lexical_documents \
         where id=1; \
         insert into lexical_documents \
         values (3,3,'new generation','new generation')",
    )?;
    assert!(store.fts_scores("opaque")?.is_empty());
    assert_eq!(store.fts_scores("generation")?.len(), 1);
    let root = d.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let profile = store.add_profile(
        "personal",
        llm_history_types::Harness::Codex,
        &root,
        None,
        None,
    )?;
    let records = llm_history_core::fixture_record(
        profile.harness,
        "native-fts-integration",
        "actual ingestion through incremental FTS",
    );
    std::fs::write(
        root.join("sessions/ingestion.jsonl"),
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(receipt.errors.is_empty(), "{receipt:?}");
    assert_eq!(store.fts_scores("ingestion")?.len(), 1);
    store
        .database
        .connection()
        .execute_batch("SET TimeZone='UTC'")?;
    let zone: String = store.database.connection().query_row(
        "select cast(timezone('America/Denver',timestamp '2026-10-01 00:00:00') as varchar)",
        [],
        |r| r.get(0),
    )?;
    assert!(zone.contains("06:00:00"));
    Ok(())
}

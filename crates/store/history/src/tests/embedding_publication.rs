use crate::{EmbeddingBatch, EmbeddingDocument, Store};
use anyhow::Result;
use llm_history_types::{Harness, query::Filter};

#[test]
fn bulk_publication_rechecks_sources_and_rolls_back_on_insert_failure() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let db = directory.path().join("history.duckdb");
    let root = directory.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let records = llm_history_core::fixture_record(Harness::Codex, "thread", "message");
    std::fs::write(
        root.join("sessions/a.jsonl"),
        records
            .iter()
            .map(|value| format!("{value}\n"))
            .collect::<String>(),
    )?;
    let store = Store::open(&db, false)?;
    store.add_profile("test", Harness::Codex, &root, None, None)?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let model = "model";
    let contract = "{\"fixture\":true}".to_owned();
    store.register_model_profile(model, &contract)?;
    let kind: String = store.database.connection().query_row(
        "select data_type from duckdb_columns() where table_name='embeddings' and column_name='vector'",
        [], |row| row.get(0),
    )?;
    assert_eq!(kind, "FLOAT[]");
    let message = store
        .messages_page(&Filter::default(), None, usize::MAX)?
        .remove(0);
    let id = message.id.parse::<i64>()?;
    let initial = EmbeddingBatch::new(&[EmbeddingDocument {
        id,
        text_hash: message.text_hash.clone(),
        vectors: vec![vec![1., 0.]],
        failure: None,
    }])?;
    assert_eq!(
        store.publish_embedding_batch(model, &contract, &initial)?,
        1
    );
    let stale = EmbeddingBatch::new(&[EmbeddingDocument {
        id,
        text_hash: message.text_hash.clone(),
        vectors: vec![vec![0., 1.]],
        failure: None,
    }])?;
    store
        .database
        .connection()
        .execute("update events set text_hash='changed' where id=?", [id])?;
    assert_eq!(store.publish_embedding_batch(model, &contract, &stale)?, 0);
    assert_eq!(
        store.embedding_vectors(model, &message)?,
        vec![vec![1., 0.]]
    );
    // Fail the bulk insert after its delete; the old vectors must survive rollback.
    store.database.connection().execute_batch(
        "ALTER TABLE embeddings RENAME TO previous_embeddings;
        CREATE TABLE embeddings (profile_id VARCHAR,message_id BIGINT,\
         text_hash VARCHAR,chunk BIGINT CHECK(chunk=0),vector FLOAT[],\
         PRIMARY KEY(profile_id,message_id,chunk));
        INSERT INTO embeddings \
         SELECT * \
         FROM previous_embeddings; \
         DROP TABLE previous_embeddings;",
    )?;
    let invalid = EmbeddingBatch::new(&[EmbeddingDocument {
        id,
        text_hash: "changed".into(),
        vectors: vec![vec![0., 1.], vec![1., 0.]],
        failure: None,
    }])?;
    assert!(
        store
            .publish_embedding_batch(model, &contract, &invalid)
            .is_err()
    );
    assert_eq!(
        store.embedding_vectors(model, &message)?,
        vec![vec![1., 0.]]
    );
    let valid = EmbeddingBatch::new(&[EmbeddingDocument {
        id,
        text_hash: "changed".into(),
        vectors: vec![vec![0., 1.]],
        failure: None,
    }])?;
    assert_eq!(store.publish_embedding_batch(model, &contract, &valid)?, 1);
    Ok(())
}

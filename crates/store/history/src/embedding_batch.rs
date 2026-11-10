use super::*;
use duckdb::types::Value as DbValue;
use std::collections::HashMap;

pub struct EmbeddingState {
    pub text_hash: String,
    pub in_scope: bool,
    pub published: bool,
}
pub struct EmbeddingDocument {
    pub id: i64,
    pub text_hash: String,
    pub vectors: Vec<Vec<f32>>,
    pub failure: Option<String>,
}
/// Bindings are prepared before acquiring the writer lock.
pub struct EmbeddingBatch {
    ids: DbValue,
    hashes: DbValue,
    failures: DbValue,
    row_ids: DbValue,
    chunks: DbValue,
    vectors: DbValue,
}
pub(crate) fn id_list(ids: &[i64]) -> DbValue {
    DbValue::List(ids.iter().copied().map(DbValue::BigInt).collect())
}
impl EmbeddingBatch {
    pub fn new(documents: &[EmbeddingDocument]) -> Result<Self> {
        let (mut ids, mut hashes, mut failures) = (Vec::new(), Vec::new(), Vec::new());
        let (mut row_ids, mut chunks, mut vectors) = (Vec::new(), Vec::new(), Vec::new());
        for document in documents {
            if document.failure.is_none() && document.vectors.is_empty() {
                bail!("cannot publish an empty embedding set");
            }
            ids.push(DbValue::BigInt(document.id));
            hashes.push(DbValue::Text(document.text_hash.clone()));
            failures.push(DbValue::Text(document.failure.clone().unwrap_or_default()));
            if document.failure.is_some() {
                continue;
            }
            for (chunk, vector) in document.vectors.iter().enumerate() {
                if vector.is_empty() || vector.iter().any(|v| !v.is_finite()) {
                    bail!("invalid staged vector");
                }
                row_ids.push(DbValue::BigInt(document.id));
                chunks.push(DbValue::BigInt(chunk as i64));
                vectors.push(float_list(vector));
            }
        }
        Ok(Self {
            ids: DbValue::List(ids),
            hashes: DbValue::List(hashes),
            failures: DbValue::List(failures),
            row_ids: DbValue::List(row_ids),
            chunks: DbValue::List(chunks),
            vectors: DbValue::List(vectors),
        })
    }
}
impl Store {
    pub fn embedding_states(
        &self,
        model: &str,
        ids: &[i64],
        profiles: &[String],
        source: Option<&str>,
    ) -> Result<HashMap<i64, EmbeddingState>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let owners = DbValue::List(
            self.profiles(profiles, false)?
                .into_iter()
                .filter(|p| source.is_none_or(|s| s == "all" || s == p.harness.key()))
                .map(|p| DbValue::Text(p.id))
                .collect(),
        );
        let mut st = self.database.connection().prepare(
            "SELECT e.id,e.text_hash,s.profile_id IN (SELECT unnest(?::VARCHAR[])),
             EXISTS(\
              SELECT 1 \
              FROM embeddings v \
              WHERE v.profile_id=? AND v.message_id=e.id \
              AND v.text_hash=e.text_hash)
             OR EXISTS(\
              SELECT 1 \
              FROM embedding_failures f \
              WHERE f.profile_id=? AND f.message_id=e.id \
              AND f.text_hash=e.text_hash)
             FROM events e JOIN sessions s ON s.id=e.session_id
             WHERE e.id IN (SELECT unnest(?::BIGINT[])) AND e.active AND e.kind='message'",
        )?;
        let rows = st.query_map(params![owners, model, model, id_list(ids)], |r| {
            Ok((
                r.get(0)?,
                EmbeddingState {
                    text_hash: r.get(1)?,
                    in_scope: r.get(2)?,
                    published: r.get(3)?,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    /// Recheck source versions and publish all selected vectors in bulk SQL.
    /// Already committed work is left unchanged on replay.
    pub fn publish_embedding_batch(
        &self,
        model: &str,
        contract: &str,
        batch: &EmbeddingBatch,
    ) -> Result<usize> {
        self.database.connection().execute_batch("BEGIN")?;
        let result = (|| -> Result<usize> {
            self.register_model_profile(model, contract)?;
            self.database.connection().execute(
                "CREATE TEMP TABLE _embedding_publish AS
                 SELECT b.* \
                  FROM (\
                  SELECT unnest(?::BIGINT[]) id,unnest(?::VARCHAR[]) text_hash,\
                  unnest(?::VARCHAR[]) failure) b
                 JOIN events e ON e.id=b.id AND e.text_hash=b.text_hash \
                  AND e.active AND e.kind='message'
                 WHERE NOT EXISTS(\
                  SELECT 1 \
                  FROM embeddings v \
                  WHERE v.profile_id=? AND v.message_id=e.id \
                  AND v.text_hash=e.text_hash)
                 AND NOT EXISTS(\
                  SELECT 1 \
                  FROM embedding_failures f \
                  WHERE f.profile_id=? AND f.message_id=e.id \
                  AND f.text_hash=e.text_hash)",
                params![batch.ids, batch.hashes, batch.failures, model, model],
            )?;
            self.database.connection().execute("DELETE FROM embeddings v USING _embedding_publish b WHERE v.profile_id=? AND v.message_id=b.id", [model])?;
            self.database.connection().execute(
                "INSERT INTO embeddings SELECT ?,r.message_id,b.text_hash,r.chunk,r.vector
                 FROM (\
                  SELECT unnest(?::BIGINT[]) message_id,unnest(?::BIGINT[]) chunk,\
                  unnest(?::FLOAT[][]) vector) r
                 JOIN _embedding_publish b ON b.id=r.message_id WHERE b.failure=''",
                params![model, batch.row_ids, batch.chunks, batch.vectors],
            )?;
            self.database.connection().execute(
                "DELETE FROM embedding_failures f USING _embedding_publish b \
                 WHERE f.profile_id=? AND f.message_id=b.id",
                [model],
            )?;
            self.database.connection().execute(
                "INSERT INTO embedding_failures \
                 SELECT ?,id,text_hash,failure \
                 FROM _embedding_publish \
                 WHERE failure<>''",
                [model],
            )?;
            let count = self.database.connection().query_row(
                "SELECT count(*) FROM _embedding_publish WHERE failure=''",
                [],
                |r| r.get(0),
            )?;
            self.database
                .connection()
                .execute_batch("DROP TABLE _embedding_publish; COMMIT")?;
            Ok(count)
        })();
        if result.is_err() {
            let _ = self.database.connection().execute_batch("ROLLBACK");
        }
        result
    }
}

//! Candidate selection happens in DuckDB; message bodies are fetched after LIMIT.
use super::*;
use duckdb::{OptionalExt, types::Value as SqlValue};
use std::collections::HashMap;

impl Store {
    pub fn create_fts(&self) -> Result<()> {
        self.database.connection().execute_batch(
            "PRAGMA create_fts_index('lexical_documents','id','body',\
             'aliases',overwrite=1,incremental=true,stemmer='none',\
             stopwords='none',ignore='[^\\p{L}\\p{N}_]+')",
        )?;
        Ok(())
    }
    pub fn fts_scores(&self, query: &str) -> Result<Vec<(String, f64)>> {
        let mut st = self.database.connection().prepare(
            "select id,score \
             from (\
             select id,fts_main_lexical_documents.match_bm25(id,?) score \
             from lexical_documents) \
             where score is not null \
             order by score desc,id",
        )?;
        Ok(st
            .query_map([query], |r| {
                Ok((r.get::<_, i64>(0)?.to_string(), r.get(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub(crate) fn retrieval_conditions(&self, f: &Filter) -> Result<queries::Conditions> {
        let mut q = self.message_conditions(f)?;
        q.sql.push(
            "NOT e.excluded AND history_message_kind(e.kind,e.excluded,\
             e.text)='conversation' AND (e.role='user' \
             OR (e.role='assistant' AND e.phase='final_answer'))"
                .into(),
        );
        Ok(q)
    }
    pub fn retrieval_counts(&self, f: &Filter, model: Option<&str>) -> Result<(usize, usize)> {
        let mut q = self.retrieval_conditions(f)?;
        let embedded = if let Some(model) = model {
            q.args.push(model.to_owned().into());
            "count(*) FILTER(\
             WHERE EXISTS(\
             SELECT 1 \
             FROM embeddings v \
             WHERE v.message_id=e.id AND v.text_hash=e.text_hash \
             AND v.profile_id=?))"
        } else {
            "0"
        };
        let query = format!(
            "SELECT count(*),{embedded} FROM {} WHERE {}",
            MESSAGE_FROM,
            q.clause()
        );
        // The embedding profile parameter occurs in SELECT before WHERE.
        if model.is_some() {
            let value = q.args.pop().unwrap();
            q.args.insert(0, value);
        }
        Ok(self
            .database
            .connection()
            .query_row(&query, duckdb::params_from_iter(q.args), |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?)
    }
    /// Messages containing every needle as a literal, case-insensitive string, in the text
    /// search covers, newest first. They tie on score, so each session is capped here;
    /// otherwise one busy session would fill the candidate limit.
    /// Capped candidates, plus how many messages matched before the caps.
    pub fn literal_candidates(
        &self,
        needles: &[String],
        f: &Filter,
        per_session: usize,
        limit: usize,
    ) -> Result<(HashMap<String, f64>, usize)> {
        anyhow::ensure!(!needles.is_empty(), "literal search needs text to match");
        let q = self.retrieval_conditions(f)?;
        let contains = " AND contains(lower(searched.body),lower(?))".repeat(needles.len());
        let sql = format!(
            "SELECT e.id,count(*) OVER () \
             FROM {} \
             JOIN lexical_documents searched ON searched.id=e.id \
             WHERE {}{contains} \
             QUALIFY row_number() OVER (\
             PARTITION BY e.session_id \
             ORDER BY e.timestamp_utc DESC NULLS LAST,e.id)<=? \
             ORDER BY e.timestamp_utc DESC NULLS LAST,e.id \
             LIMIT ?",
            MESSAGE_FROM,
            q.clause()
        );
        let mut args = q.args;
        args.extend(needles.iter().cloned().map(SqlValue::Text));
        args.push(SqlValue::BigInt(per_session as i64));
        args.push(SqlValue::BigInt(limit as i64));
        let rows = self
            .database
            .connection()
            .prepare(&sql)?
            .query_map(duckdb::params_from_iter(args), |r| {
                Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, i64>(1)?))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let matched = rows.first().map_or(0, |(_, matched)| *matched as usize);
        Ok((rows.into_iter().map(|(id, _)| (id, 1.0)).collect(), matched))
    }
    pub fn lexical_candidates(
        &self,
        query: &str,
        f: &Filter,
        limit: usize,
    ) -> Result<(HashMap<String, f64>, String)> {
        let q = self.retrieval_conditions(f)?;
        let sql = format!(
            "WITH scored AS (\
             SELECT e.id,e.timestamp_utc,\
             fts_main_lexical_documents.match_bm25(e.id,?) score \
             FROM {} \
             WHERE {}) \
             SELECT id,score \
             FROM scored \
             WHERE score IS NOT NULL \
             ORDER BY score DESC,timestamp_utc DESC NULLS LAST,id \
             LIMIT ?",
            MESSAGE_FROM,
            q.clause()
        );
        let mut args = vec![SqlValue::Text(format!("{query} {}", code_aliases(query)))];
        args.extend(q.args.clone());
        args.push(SqlValue::BigInt(limit as i64));
        match self.database.connection().prepare(&sql) {
            Ok(mut st) => {
                let scored = st
                    .query_map(duckdb::params_from_iter(args), |r| {
                        Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, f64>(1)?))
                    })?
                    .collect::<duckdb::Result<HashMap<_, _>>>()?;
                return Ok((scored, "duckdb-fts".into()));
            }
            Err(error) => {
                let indexed: bool = self.database.connection().query_row(
                    "SELECT EXISTS(\
                     SELECT 1 \
                     FROM information_schema.schemata \
                     WHERE schema_name='fts_main_lexical_documents')",
                    [],
                    |r| r.get(0),
                )?;
                if indexed {
                    return Err(error.into());
                }
            }
        }
        // Match Python's plain substring fallback when no FTS index exists.
        let sql = format!(
            "SELECT e.id,1.0 score \
             FROM {} \
             WHERE {} AND contains(lower(e.text),lower(?)) \
             ORDER BY e.timestamp_utc DESC NULLS LAST,src.uri,e.ordinal \
             LIMIT ?",
            MESSAGE_FROM,
            q.clause()
        );
        let mut args = q.args;
        args.push(SqlValue::Text(query.into()));
        args.push(SqlValue::BigInt(limit as i64));
        let scores = self
            .database
            .connection()
            .prepare(&sql)?
            .query_map(duckdb::params_from_iter(args), |r| {
                Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, f64>(1)?))
            })?
            .collect::<duckdb::Result<HashMap<_, _>>>()?;
        Ok((scores, "sql-lexical".into()))
    }
    pub fn semantic_candidates(
        &self,
        model: &str,
        vector: &[f32],
        f: &Filter,
        limit: usize,
    ) -> Result<HashMap<String, f64>> {
        let q = self.retrieval_conditions(f)?;
        let sql = format!(
            "SELECT e.id,max(list_cosine_similarity(v.vector,\
             ?::FLOAT[])) score \
             FROM {} \
             JOIN embeddings v ON v.message_id=e.id \
             AND v.text_hash=e.text_hash \
             WHERE v.profile_id=? AND {} \
             GROUP BY e.id \
             ORDER BY score DESC,max(e.timestamp_utc) DESC NULLS LAST,e.id \
             LIMIT ?",
            MESSAGE_FROM,
            q.clause()
        );
        let mut args = vec![float_list(vector), SqlValue::Text(model.into())];
        args.extend(q.args);
        args.push(SqlValue::BigInt(limit as i64));
        Ok(self
            .database
            .connection()
            .prepare(&sql)?
            .query_map(duckdb::params_from_iter(args), |r| {
                Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, f64>(1)?))
            })?
            .collect::<duckdb::Result<HashMap<_, _>>>()?)
    }

    /// Rank current document vectors in the anchor's store unless the caller selects stores.
    pub fn similar_messages(
        &self,
        message_no: i64,
        model: &str,
        filter: &Filter,
        limit: usize,
        per_session: usize,
        other_sessions: bool,
    ) -> Result<Vec<(Message, f64)>> {
        let con = self.database.connection();
        let (event, session, owner, has_vector): (i64, i64, String, bool) = con
            .query_row(
                "SELECT e.id,e.session_id,s.profile_id,EXISTS(SELECT 1 FROM embeddings v \
                 WHERE v.message_id=e.id AND v.text_hash=e.text_hash AND v.profile_id=?) \
                 FROM message_numbers n JOIN events e ON e.id=n.event_id \
                 JOIN sessions s ON s.id=e.session_id WHERE n.message_no=? AND e.active \
                 AND e.kind='message' AND e.text<>''",
                params![model, message_no],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
            .context("message not found")?;
        anyhow::ensure!(
            has_vector,
            "message has no current embedding for this model; run semantic backfill for its profile"
        );

        self.validate_project_filter(filter)?;
        let mut q = self.retrieval_conditions(filter)?;
        if filter.profiles.is_empty() {
            q.add("s.profile_id=?", owner);
        }
        if other_sessions {
            q.add("e.session_id<>?", session);
        }
        let sql = format!(
            "WITH seed AS MATERIALIZED (
                SELECT v.vector FROM embeddings v JOIN events e
                ON e.id=v.message_id AND e.text_hash=v.text_hash
                WHERE v.message_id=? AND v.profile_id=?
             ), scored AS (
                SELECT e.id,e.session_id,e.timestamp_utc,max(list_cosine_similarity(v.vector,z.vector)) score
                FROM {MESSAGE_FROM}
                JOIN embeddings v ON v.message_id=e.id AND v.text_hash=e.text_hash
                JOIN seed z ON len(z.vector)=len(v.vector)
                WHERE v.profile_id=? AND e.id<>? AND {}
                GROUP BY e.id,e.session_id,e.timestamp_utc
             )
             SELECT id,score FROM scored
             QUALIFY row_number() OVER (PARTITION BY session_id
                ORDER BY score DESC,timestamp_utc DESC NULLS LAST,id)<=?
             ORDER BY score DESC,timestamp_utc DESC NULLS LAST,id LIMIT ?",
            q.clause()
        );
        let mut args = vec![
            event.into(),
            model.to_owned().into(),
            model.to_owned().into(),
            event.into(),
        ];
        args.extend(q.args);
        args.extend([
            SqlValue::BigInt(per_session as i64),
            SqlValue::BigInt(limit as i64),
        ]);
        let ranks = self.database.with_query_timeout(
            std::time::Duration::from_secs(llm_history_types::storage::DEFAULT_SQL_TIMEOUT_SECONDS),
            |_| {
                Ok(con
                    .prepare(&sql)?
                    .query_map(duckdb::params_from_iter(args), |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
                    })?
                    .collect::<duckdb::Result<Vec<_>>>()?)
            },
        )?;
        let ids = ranks.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let mut messages = self
            .messages_by_ids(&ids, filter)?
            .into_iter()
            .map(|message| (message.id.clone(), message))
            .collect::<HashMap<_, _>>();
        ranks
            .into_iter()
            .map(|(id, score)| {
                Ok((
                    messages
                        .remove(&id.to_string())
                        .context("ranked message missing")?,
                    score,
                ))
            })
            .collect()
    }

    pub fn messages_by_ids(&self, ids: &[i64], f: &Filter) -> Result<Vec<Message>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut q = self.retrieval_conditions(f)?;
        q.add(
            "e.id IN (SELECT unnest(?::BIGINT[]))",
            embedding_batch::id_list(ids),
        );
        let query = message_query(&self.harness_sql()?);
        Ok(self
            .database
            .connection()
            .prepare(&format!("{query} WHERE {} ORDER BY e.id", q.clause()))?
            .query_map(duckdb::params_from_iter(q.args), message_row)?
            .collect::<duckdb::Result<Vec<_>>>()?)
    }
    pub fn answer_messages(&self, ids: &[i64], f: &Filter) -> Result<HashMap<String, Message>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let q = self.retrieval_conditions(f)?;
        let sql = format!(
            "WITH picked AS MATERIALIZED (
                SELECT id,source_id,session_id,ordinal FROM events
                WHERE id IN (SELECT unnest(?::BIGINT[])) AND role='user'
             ), affected AS MATERIALIZED (
                SELECT DISTINCT source_id,session_id FROM picked
             ), next_users AS (
                SELECT e.source_id,e.session_id,e.ordinal FROM events e
                SEMI JOIN affected f USING (source_id,session_id)
                WHERE e.active AND e.kind='message' AND e.role='user' AND e.text<>'' AND NOT e.excluded
             ), bounds AS (
                SELECT u.*,n.ordinal next_user FROM picked u ASOF LEFT JOIN next_users n
                ON u.source_id=n.source_id AND u.session_id=n.session_id AND u.ordinal<n.ordinal
             ), answers AS (
                SELECT e.id,e.source_id,e.session_id,e.ordinal FROM {}
                SEMI JOIN affected f ON f.source_id=e.source_id AND f.session_id=e.session_id
                WHERE {} AND e.role='assistant' AND e.phase='final_answer'
             )
             SELECT b.id,a.id FROM bounds b ASOF JOIN answers a
             ON b.source_id=a.source_id AND b.session_id=a.session_id AND b.ordinal<a.ordinal
             WHERE b.next_user IS NULL OR a.ordinal<=b.next_user",
            MESSAGE_FROM,q.clause()
        );
        let mut args = vec![embedding_batch::id_list(ids)];
        args.extend(q.args);
        let pairs = self
            .database
            .connection()
            .prepare(&sql)?
            .query_map(duckdb::params_from_iter(args), |r| {
                Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, i64>(1)?))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let answer_ids = pairs.iter().map(|(_, id)| *id).collect::<Vec<_>>();
        let messages = self
            .messages_by_ids(&answer_ids, f)?
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect::<HashMap<_, _>>();
        Ok(pairs
            .into_iter()
            .filter_map(|(user, id)| messages.get(&id.to_string()).map(|m| (user, m.clone())))
            .collect())
    }
}

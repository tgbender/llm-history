//! Embedding coverage, stored vectors and failures, scored in DuckDB.
use super::*;

impl Store {
    pub fn model_contract(&self, id: &str) -> Result<String> {
        Ok(self.database.connection().query_row(
            "select data from model_profiles where id=?",
            [id],
            |row| row.get(0),
        )?)
    }
    pub fn register_model_profile(&self, id: &str, data: &str) -> Result<()> {
        if let Ok(old) = self.database.connection().query_row(
            "select data from model_profiles where id=?",
            [id],
            |r| r.get::<_, String>(0),
        ) && old != data
        {
            bail!("model profile contract changed without a new profile ID");
        }
        self.database.connection().execute(
            "insert into model_profiles values (?,?) on conflict do nothing",
            params![id, data],
        )?;
        Ok(())
    }
    pub fn coverage(&self, profile: &str, names: &[String]) -> Result<Value> {
        let selected = self
            .profiles(names, false)?
            .into_iter()
            .map(|p| p.id)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Ok(json!({"eligible":0,"current":0,"pending":0,"failed":0}));
        }
        let placeholders = vec!["?"; selected.len()].join(",");
        let query = format!(
            "WITH eligible AS (\
             SELECT e.id,e.text_hash \
             FROM events e \
             JOIN sessions s ON s.id=e.session_id \
             WHERE e.active AND e.kind='message' AND NOT e.excluded \
             AND e.text<>'' AND (e.role='user' OR (e.role='assistant' \
             AND e.phase='final_answer')) AND NOT (s.subagent \
             AND e.role='user') AND NOT coalesce(e.agent_protocol,false) \
             AND s.profile_id IN ({placeholders})) \
             SELECT count(*),count(*) FILTER(\
             WHERE EXISTS(\
             SELECT 1 \
             FROM embeddings v \
             WHERE v.profile_id=? AND v.message_id=e.id \
             AND v.text_hash=e.text_hash)),count(*) FILTER(\
             WHERE EXISTS(\
             SELECT 1 \
             FROM embedding_failures f \
             WHERE f.profile_id=? AND f.message_id=e.id \
             AND f.text_hash=e.text_hash)) \
             FROM eligible e"
        );
        let mut args = selected;
        args.push(profile.into());
        args.push(profile.into());
        let (n, current, failed): (i64, i64, i64) =
            self.database
                .connection()
                .query_row(&query, duckdb::params_from_iter(args), |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?;
        Ok(
            json!({"eligible":n,"current":current,"pending":(n-current-failed).max(0),"failed":failed}),
        )
    }
    pub fn embedding_vectors(&self, profile: &str, m: &Message) -> Result<Vec<Vec<f32>>> {
        let mut st=self.database.connection().prepare("select vector from embeddings where profile_id=? and message_id=? and text_hash=? order by chunk")?;
        let rows = st.query_map(params![profile, m.id.parse::<i64>()?, m.text_hash], |r| {
            r.get::<_, duckdb::types::Value>(0)
        })?;
        rows.map(|r| match r? {
            duckdb::types::Value::List(items) => items
                .into_iter()
                .map(|v| match v {
                    duckdb::types::Value::Float(f) => Ok(f),
                    _ => bail!("embedding element is not FLOAT"),
                })
                .collect(),
            _ => bail!("embedding is not a FLOAT[] list"),
        })
        .collect()
    }
    /// Best chunk similarity per current message, scored in one DuckDB query.
    pub fn semantic_scores(
        &self,
        profile: &str,
        query: &[f32],
        eligible_ids: &[i64],
    ) -> Result<std::collections::HashMap<String, f64>> {
        if eligible_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let ids = duckdb::types::Value::List(
            eligible_ids
                .iter()
                .copied()
                .map(duckdb::types::Value::BigInt)
                .collect(),
        );
        let mut st = self.database.connection().prepare(
            "select v.message_id, max(list_cosine_similarity(v.vector, ?::FLOAT[])) \
             from embeddings v \
             join events e on e.id=v.message_id and e.text_hash=v.text_hash \
             where v.profile_id=? and e.active and v.message_id in (\
             select unnest(?::BIGINT[])) \
             group by v.message_id",
        )?;
        let rows = st.query_map(params![float_list(query), profile, ids], |r| {
            Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, f64>(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn has_failure(&self, profile: &str, m: &Message) -> Result<bool> {
        Ok(self.database.connection().query_row("select count(*)>0 from embedding_failures where profile_id=? and message_id=? and text_hash=?",params![profile,m.id.parse::<i64>()?,m.text_hash],|r|r.get(0))?)
    }
    pub fn retry_failures(
        &self,
        model: &str,
        profiles: &[String],
        source: Option<&str>,
    ) -> Result<()> {
        let ids = self
            .profiles(profiles, false)?
            .into_iter()
            .filter(|p| source.is_none_or(|s| s == "all" || s == p.harness.key()))
            .map(|p| format!("'{}'", p.id.replace('\'', "''")))
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            self.database.connection().execute(
                &format!(
                    "delete from embedding_failures \
                     where profile_id=? and message_id in (\
                     select e.id \
                     from events e \
                     join sessions s on s.id=e.session_id \
                     where s.profile_id in ({}))",
                    ids.join(",")
                ),
                [model],
            )?;
        }
        Ok(())
    }
}

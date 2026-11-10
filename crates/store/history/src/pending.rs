use super::*;

pub struct PendingPage {
    pub messages: Vec<Message>,
    pub last_id: i64,
    pub oversized: usize,
}
impl Store {
    fn pending_predicate(
        &self,
        profiles: &[String],
        source: Option<&str>,
    ) -> Result<Option<String>> {
        let ids = self
            .profiles(profiles, false)?
            .into_iter()
            .filter(|p| source.is_none_or(|s| s == "all" || s == p.harness.key()))
            .map(|p| format!("'{}'", p.id.replace('\'', "''")))
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(None);
        }
        Ok(Some(format!(
            "e.active AND e.kind='message' AND NOT e.excluded AND e.text<>'' \
             AND (e.role='user' OR (e.role='assistant' \
             AND e.phase='final_answer')) AND NOT (s.subagent \
             AND e.role='user') AND NOT coalesce(e.agent_protocol,false) \
             AND s.profile_id IN ({}) AND NOT EXISTS (\
             SELECT 1 \
             FROM embeddings v \
             WHERE v.profile_id=? AND v.message_id=e.id \
             AND v.text_hash=e.text_hash) AND NOT EXISTS (\
             SELECT 1 \
             FROM embedding_failures f \
             WHERE f.profile_id=? AND f.message_id=e.id \
             AND f.text_hash=e.text_hash)",
            ids.join(",")
        )))
    }
    pub fn has_pending_messages(
        &self,
        model: &str,
        profiles: &[String],
        source: Option<&str>,
    ) -> Result<bool> {
        let Some(predicate) = self.pending_predicate(profiles, source)? else {
            return Ok(false);
        };
        Ok(self.database.connection().query_row(
            &format!(
                "SELECT EXISTS(\
                 SELECT 1 \
                 FROM events e \
                 JOIN sessions s ON s.id=e.session_id \
                 WHERE {predicate} \
                 LIMIT 1)"
            ),
            params![model, model],
            |r| r.get(0),
        )?)
    }
    /// Keyset pagination bounds text materialization independently of corpus size.
    #[allow(clippy::too_many_arguments)]
    pub fn pending_messages(
        &self,
        model: &str,
        profiles: &[String],
        source: Option<&str>,
        after: i64,
        limit: usize,
        max_chars: usize,
        staged: &[i64],
    ) -> Result<PendingPage> {
        if limit == 0 || max_chars == 0 {
            bail!("pending-page limits must be positive");
        }
        let Some(predicate) = self.pending_predicate(profiles, source)? else {
            return Ok(PendingPage {
                messages: vec![],
                last_id: after,
                oversized: 0,
            });
        };
        let message_query = message_query(&self.harness_sql()?);
        let mut st = self.database.connection().prepare(&format!("{message_query} WHERE {predicate} AND e.id>? AND length(e.text)<=? AND e.id NOT IN (SELECT unnest(?::BIGINT[])) ORDER BY e.id LIMIT ?"))?;
        let rows = st.query_map(
            params![
                model,
                model,
                after,
                max_chars as u64,
                super::embedding_batch::id_list(staged),
                limit as u64
            ],
            message_row,
        )?;
        let mut messages = Vec::new();
        let mut last_id = after;
        for row in rows {
            let message = row?;
            last_id = message.id.parse()?;
            messages.push(message);
        }
        // Oversized documents only matter once nothing else is pending, so count them then.
        let oversized = if messages.is_empty() {
            self.database.connection().query_row(
                &format!(
                    "SELECT count(*) \
                     FROM events e \
                     JOIN sessions s ON s.id=e.session_id \
                     WHERE {predicate} AND length(e.text)>?"
                ),
                params![model, model, max_chars as u64],
                |r| r.get::<_, usize>(0),
            )?
        } else {
            0
        };
        Ok(PendingPage {
            messages,
            last_id,
            oversized,
        })
    }
}

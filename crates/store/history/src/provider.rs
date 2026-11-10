//! Sparse provider facts: turn contexts and token counts. Timestamps and turn IDs
//! are columns of the fact table itself.
use super::*;
use llm_history_types::provider::Detail;

pub(crate) struct ProviderBatch {
    rows: Vec<(i64, Detail)>,
}
impl ProviderBatch {
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }
    pub fn push(&mut self, id: i64, detail: Detail) {
        self.rows.push((id, detail));
    }
    /// Replaces the facts of every staged record, including ones that lost them.
    pub fn flush(&mut self, store: &Store) -> Result<()> {
        let con = &store.database.connection();
        con.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS stage_provider_context AS \
             SELECT * \
             FROM provider_turn_contexts \
             LIMIT 0;\
             CREATE TEMP TABLE IF NOT EXISTS stage_provider_tokens AS \
             SELECT * \
             FROM provider_token_events \
             LIMIT 0;",
        )?;
        let mut contexts =
            con.appender_to_catalog_and_db("stage_provider_context", "temp", "main")?;
        let mut tokens = con.appender_to_catalog_and_db("stage_provider_tokens", "temp", "main")?;
        for (id, d) in self.rows.drain(..) {
            if let Some(c) = d.context {
                contexts.append_row(params![
                    id,
                    c.cwd,
                    c.current_date,
                    c.timezone,
                    c.model,
                    c.effort,
                    c.approval_policy,
                    c.sandbox_policy,
                    c.personality,
                    c.service_tier,
                    c.collaboration_mode_json,
                    c.workspace_roots_json
                ])?;
            }
            if let Some(t) = d.tokens {
                tokens.append_row(params![
                    id,
                    t.ordinal,
                    t.context_line,
                    t.total.input,
                    t.total.cached,
                    t.total.cache_write,
                    t.total.output,
                    t.total.reasoning,
                    t.total.total,
                    t.last.input,
                    t.last.cached,
                    t.last.cache_write,
                    t.last.output,
                    t.last.reasoning,
                    t.last.total,
                    t.model_context_window,
                    t.limit_id
                ])?;
            }
        }
        drop((contexts, tokens));
        con.execute_batch(
            "DELETE FROM provider_turn_contexts \
             WHERE event_id IN (\
             SELECT id \
             FROM temp.stage_events \
             WHERE fact_index=0);\
             INSERT INTO provider_turn_contexts \
             SELECT * \
             FROM temp.stage_provider_context;
         DELETE FROM provider_token_events \
          WHERE event_id IN (\
          SELECT id \
          FROM temp.stage_events \
          WHERE fact_index=0);\
          INSERT INTO provider_token_events \
          SELECT * \
          FROM temp.stage_provider_tokens;
         DELETE FROM temp.stage_provider_context;DELETE FROM temp.stage_provider_tokens;",
        )?;
        Ok(())
    }
}

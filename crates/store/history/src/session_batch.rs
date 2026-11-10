//! Ordered session metadata changes, folded once per projection flush.
use super::*;
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct SessionBatch {
    slots: HashMap<(String, String), usize>,
    keys: Vec<(String, String)>,
    last: Vec<usize>,
    updates: Vec<(usize, Harness, SessionUpdate)>,
}

impl SessionBatch {
    pub(crate) fn len(&self) -> usize {
        self.updates.len()
    }

    pub(crate) fn push(&mut self, profile: &Profile, update: SessionUpdate) -> (usize, usize) {
        let key = (profile.id.clone(), update.session.clone());
        let slot = *self.slots.entry(key.clone()).or_insert_with(|| {
            self.keys.push(key);
            self.last.push(usize::MAX);
            self.keys.len() - 1
        });
        if let Some((_, _, prior)) = self.updates.get(self.last[slot])
            && same_metadata(prior, &update)
        {
            return (slot, 0);
        }
        let c = &update.canonical;
        let bytes = update.session.len()
            + update.cwd.len()
            + update.title.len()
            + update.model.len()
            + update.parent.len()
            + update.relation.len()
            + update.last_prompt.len()
            + update.first_user_message.len()
            + update.timezone.len()
            + c.metadata.len()
            + c.repository_root.len()
            + c.runtime_id.len()
            + c.agent_label.len()
            + c.kind.len()
            + c.native_kind.len()
            + c.evidence.len()
            + c.started.len()
            + 128;
        self.last[slot] = self.updates.len();
        self.updates.push((slot, profile.harness, update));
        (slot, bytes)
    }

    pub(crate) fn publish(&mut self, store: &Store) -> Result<Vec<i64>> {
        if self.keys.is_empty() {
            return Ok(Vec::new());
        }
        let con = store.database.connection();
        con.execute_batch(
            "CREATE OR REPLACE TEMP TABLE stage_session_keys (slot BIGINT,profile_id VARCHAR,native_id VARCHAR);
             CREATE OR REPLACE TEMP TABLE stage_session_updates (
               pos BIGINT,id BIGINT,profile_id VARCHAR,harness VARCHAR,native_id VARCHAR,
               cwd VARCHAR,title VARCHAR,model VARCHAR,parent VARCHAR,relation VARCHAR,subagent BOOLEAN,
               project VARCHAR,runtime VARCHAR,kind VARCHAR,native_kind VARCHAR,evidence VARCHAR,
               started VARCHAR,repository VARCHAR,label VARCHAR,metadata VARCHAR
             )",
        )?;
        {
            let mut app = con.appender_to_catalog_and_db("stage_session_keys", "temp", "main")?;
            for (slot, (profile, native)) in self.keys.iter().enumerate() {
                app.append_row(params![slot as i64, profile, native])?;
            }
            app.flush()?;
        }
        let existing = {
            let mut st = con.prepare(
                "SELECT s.id,coalesce(thread_title(n.name,n.title,n.preview,n.first_user_message,n.agent_path,n.source),''),
                        coalesce(n.model,'')
                 FROM temp.stage_session_keys k
                 LEFT JOIN sessions s USING (profile_id,native_id)
                 LEFT JOIN native_threads n ON n.profile_id=k.profile_id AND n.id=k.native_id
                 ORDER BY k.slot",
            )?;
            st.query_map([], |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?
        };
        let mut new_ids =
            crate::batch::allocate(con, existing.iter().filter(|row| row.0.is_none()).count())?
                .into_iter();
        let ids: Vec<_> = existing
            .iter()
            .map(|row| row.0.or_else(|| new_ids.next()).context("session ID"))
            .collect::<Result<_>>()?;
        {
            let mut app =
                con.appender_to_catalog_and_db("stage_session_updates", "temp", "main")?;
            for (pos, (slot, harness, s)) in self.updates.drain(..).enumerate() {
                let c = &s.canonical;
                let (_, native_title, native_model) = &existing[slot];
                let title = [
                    &s.title[..],
                    native_title,
                    if c.kind == "guardian" {
                        "Guardian review"
                    } else {
                        ""
                    },
                    &s.last_prompt,
                    &s.first_user_message,
                ]
                .into_iter()
                .find(|t| !t.is_empty())
                .map(llm_history_core::title_line)
                .unwrap_or_default();
                let model = if s.model.is_empty() {
                    native_model
                } else {
                    &s.model
                };
                let runtime = if c.runtime_id.is_empty() {
                    if harness == Harness::Claude && s.subagent {
                        &s.parent
                    } else {
                        &s.session
                    }
                } else {
                    &c.runtime_id
                };
                let kind = if c.kind.is_empty() {
                    if s.subagent {
                        "subagent"
                    } else {
                        "interactive"
                    }
                } else {
                    &c.kind
                };
                let evidence = if c.evidence.is_empty() {
                    "adapter_inferred"
                } else {
                    &c.evidence
                };
                let started = llm_history_core::provider::timestamp(&c.started, &s.timezone).utc;
                app.append_row(params![
                    pos as i64,
                    ids[slot],
                    self.keys[slot].0,
                    harness.key(),
                    s.session,
                    s.cwd,
                    title,
                    model,
                    s.parent,
                    s.relation,
                    s.subagent,
                    project_name(&s.cwd),
                    not_empty(runtime),
                    kind,
                    not_empty(&c.native_kind),
                    evidence,
                    started,
                    not_empty(&c.repository_root),
                    not_empty(&c.agent_label),
                    not_empty(&c.metadata)
                ])?;
            }
            app.flush()?;
        }
        con.execute_batch(include_str!("session_batch.sql"))?;
        con.execute(
            "UPDATE canonical_session_relations r SET active=false,updated_at=now()
             FROM temp.stage_session_latest s
             WHERE r.source_session_id=s.id AND r.active
             AND (r.target_native_session_id<>s.parent OR r.relation_kind<>coalesce(nullif(s.relation,''),'parent') OR s.parent='')",
            [],
        )?;
        con.execute(
            "INSERT INTO canonical_session_relations
             SELECT nextval('entity_ids'),r.id,NULL,r.parent,r.relation,r.native_kind,
                    CASE WHEN r.harness='claude' THEN 'native_path' ELSE 'native_explicit' END,
                    NULL,NULL,r.pos=s.pos,now(),now()
             FROM (
                 SELECT id,pos,harness,parent,coalesce(nullif(relation,''),'parent') relation,native_kind
                 FROM temp.stage_session_updates WHERE parent<>'' AND parent<>native_id
                 QUALIFY row_number() OVER (PARTITION BY id,parent,coalesce(nullif(relation,''),'parent') ORDER BY pos DESC)=1
             ) r JOIN temp.stage_session_latest s USING (id)
             WHERE NOT EXISTS (
                 SELECT 1 FROM canonical_session_relations old
                 WHERE old.source_session_id=r.id AND old.target_native_session_id=r.parent
                 AND old.relation_kind=r.relation AND old.active=(r.pos=s.pos)
                 AND old.native_relation_kind IS NOT DISTINCT FROM r.native_kind
             )
             ON CONFLICT (source_session_id,relation_kind,target_native_session_id)
             DO UPDATE SET native_relation_kind=excluded.native_relation_kind,
                           evidence_kind=excluded.evidence_kind,active=excluded.active,updated_at=now()",
            [],
        )?;
        con.execute_batch(
            "DROP TABLE temp.stage_session_keys; DROP TABLE temp.stage_session_updates;
             DROP TABLE temp.stage_session_latest;",
        )?;
        self.keys.clear();
        self.slots.clear();
        self.last.clear();
        Ok(ids)
    }
}

// Match the metadata change detection used by the former single-session writer.
fn same_metadata(a: &SessionUpdate, b: &SessionUpdate) -> bool {
    a.cwd == b.cwd
        && a.title == b.title
        && a.last_prompt == b.last_prompt
        && a.first_user_message == b.first_user_message
        && a.model == b.model
        && a.parent == b.parent
        && a.relation == b.relation
        && a.subagent == b.subagent
        && a.canonical.revision == b.canonical.revision
}

fn not_empty(s: &str) -> Option<&str> {
    if s.is_empty() { None } else { Some(s) }
}

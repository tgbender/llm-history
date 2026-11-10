//! Canonical sessions and record-level projections. Message text remains shared
//! with the occurrence index; relationships and sequence ranges are store-scoped.
use super::*;
use std::collections::BTreeMap;

impl Store {
    /// Retires the projection of sources about to be rebuilt. Fact rows stay, inactive,
    /// so their IDs are reused; derived side tables are deleted in one statement each.
    pub(crate) fn clear_sources(&self, sources: &[i64]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }
        let ids = embedding_batch::id_list(sources);
        for sql in [
            "DELETE FROM provider_token_events \
             WHERE event_id IN (\
             SELECT id \
             FROM events \
             WHERE source_id IN (\
             SELECT unnest(?::BIGINT[])))",
            "DELETE FROM provider_turn_contexts \
             WHERE event_id IN (\
             SELECT id \
             FROM events \
             WHERE source_id IN (\
             SELECT unnest(?::BIGINT[])))",
            "DELETE FROM canonical_event_relations WHERE source_id IN (SELECT unnest(?::BIGINT[]))",
            "UPDATE canonical_session_sources SET active=false WHERE source_id IN (SELECT unnest(?::BIGINT[]))",
            "UPDATE events SET active=false WHERE source_id IN (SELECT unnest(?::BIGINT[]))",
            "DELETE FROM lexical_documents WHERE source_id IN (SELECT unnest(?::BIGINT[]))",
        ] {
            self.database.connection().execute(sql, [&ids])?;
        }
        self.reset_session_starts(sources)
    }

    /// Sessions whose every source is being rebuilt take their start from the
    /// rebuilt sources alone, so a stale earlier start does not survive.
    pub(crate) fn reset_session_starts(&self, sources: &[i64]) -> Result<()> {
        self.database.connection().execute(
            "UPDATE canonical_sessions \
             SET started_at_utc=NULL \
             WHERE id IN (\
             SELECT session_id \
             FROM canonical_session_sources \
             GROUP BY session_id \
             HAVING bool_and(list_contains(?::BIGINT[],source_id)))",
            [embedding_batch::id_list(sources)],
        )?;
        Ok(())
    }

    pub(crate) fn start_projection(&self) -> Result<()> {
        self.database.connection().execute_batch(
            "CREATE OR REPLACE TEMP TABLE projection_epoch_tail (
                session_id BIGINT, source_id BIGINT, next_epoch BIGINT,
                PRIMARY KEY (session_id, source_id)
             )",
        )?;
        Ok(())
    }

    /// Derives sequence, epoch and record columns for the staged batch and writes it.
    pub(crate) fn project_events(&self) -> Result<()> {
        self.database
            .connection()
            .execute_batch(include_str!("canonical_batch.sql"))?;
        Ok(())
    }

    /// Completes sources whose facts are all written: session ownership, epoch bounds,
    /// session times and native parent links, each as one statement for the batch.
    pub(crate) fn finish_sources(
        &self,
        finished: &[(i64, &Profile, &SessionUpdate)],
    ) -> Result<()> {
        let con = self.database.connection();
        if finished.is_empty() {
            con.execute_batch("DROP TABLE temp.projection_epoch_tail")?;
            return Ok(());
        }
        self.database.connection().execute_batch(
            "CREATE OR REPLACE TEMP TABLE finished_sources(source_id BIGINT,\
             store_id VARCHAR,native_session_id VARCHAR,started VARCHAR,\
             owner BIGINT)",
        )?;
        {
            let mut app = self.database.connection().appender_to_catalog_and_db(
                "finished_sources",
                "temp",
                "main",
            )?;
            for (source, profile, parser) in finished {
                let started = llm_history_core::provider::timestamp(
                    &parser.canonical.started,
                    &parser.timezone,
                )
                .utc;
                app.append_row(params![
                    source,
                    profile.id,
                    parser.session,
                    started,
                    None::<i64>
                ])?;
            }
        }
        self.database.connection().execute_batch(
            "UPDATE temp.finished_sources f \
             SET owner=s.id \
             FROM sessions s \
             WHERE s.profile_id=f.store_id \
             AND s.native_id=f.native_session_id",
        )?;
        con.execute_batch(
            "CREATE OR REPLACE TEMP TABLE mixed_sources AS
             SELECT DISTINCT f.source_id,f.owner
             FROM temp.finished_sources f
             JOIN events e ON e.source_id=f.source_id
             WHERE e.fact_index=0 AND e.active AND e.session_seq IS NOT NULL
             AND e.session_id<>f.owner",
        )?;
        if con.query_row(
            "SELECT EXISTS (SELECT 1 FROM temp.mixed_sources)",
            [],
            |r| r.get(0),
        )? {
            // Metadata can follow introductory records. Move the source's
            // earlier records to its resolved owner without changing their IDs.
            crate::batch::create_stage(con)?;
            con.execute_batch(&format!(
                "UPDATE events e SET session_id=m.owner FROM temp.mixed_sources m
                 WHERE e.source_id=m.source_id AND e.active;
                 {} WHERE active AND source_id IN (SELECT source_id FROM temp.mixed_sources) ORDER BY source_id;
                 DELETE FROM canonical_event_relations r USING temp.mixed_sources m
                 WHERE r.source_id=m.source_id;
                 DELETE FROM canonical_session_sources s USING temp.mixed_sources m
                 WHERE s.source_id=m.source_id AND s.session_id<>m.owner;
                 DELETE FROM temp.projection_epoch_tail t USING temp.mixed_sources m
                 WHERE t.source_id=m.source_id;",
                crate::batch::RESTAGE_EVENTS
            ))?;
            self.project_events()?;
            con.execute_batch(
                "DELETE FROM temp.stage_events;
                 UPDATE canonical_sessions s SET active=false
                 WHERE s.active AND NOT EXISTS (
                     SELECT 1 FROM events e WHERE e.session_id=s.id
                     AND e.fact_index=0 AND e.active AND e.session_seq IS NOT NULL
                 )",
            )?;
        }
        con.execute_batch("DROP TABLE temp.mixed_sources; DROP TABLE temp.projection_epoch_tail;")?;
        self.database.connection().execute_batch(
            "
            CREATE OR REPLACE TEMP TABLE finished_sessions AS \
             SELECT DISTINCT session_id \
             FROM canonical_session_sources \
             WHERE source_id IN (\
             SELECT source_id \
             FROM temp.finished_sources);
            UPDATE canonical_context_epochs ep \
             SET active=b.first_id IS NOT NULL,starts_at_event_id=b.first_id,\
             ends_at_event_id=b.last_id,updated_at=now()
             FROM (\
              SELECT ep.id,arg_min(e.id,e.session_seq) first_id,arg_max(e.id,\
              e.session_seq) last_id \
              FROM canonical_context_epochs ep \
              LEFT JOIN events e ON e.context_epoch_id=ep.id \
              AND e.fact_index=0 AND e.active AND e.session_seq IS NOT NULL \
              WHERE ep.session_id IN (\
              SELECT session_id \
              FROM temp.finished_sessions) \
              GROUP BY ep.id) b
             WHERE ep.id=b.id AND (ep.starts_at_event_id IS DISTINCT FROM b.first_id \
              OR ep.ends_at_event_id IS DISTINCT FROM b.last_id \
              OR ep.active IS DISTINCT FROM (b.first_id IS NOT NULL));
            UPDATE provider_turn_contexts c \
             SET cwd=s.cwd \
             FROM events e,sessions s,canonical_sessions cs \
             WHERE c.event_id=e.id AND s.id=e.session_id AND cs.id=s.id \
             AND cs.harness_key='claude' AND e.source_id IN (\
             SELECT source_id \
             FROM temp.finished_sources) AND c.cwd='' AND s.cwd<>'';
            -- A session continued across several files starts with its earliest one,
            -- recomputed so a rebuilt file cannot leave a stale earlier start behind.
            UPDATE canonical_session_sources c \
             SET started_at_utc=try_cast(f.started AS TIMESTAMPTZ) \
             FROM temp.finished_sources f \
             WHERE c.source_id=f.source_id AND c.session_id=f.owner \
             AND c.started_at_utc IS DISTINCT FROM try_cast(f.started AS TIMESTAMPTZ);
            UPDATE canonical_sessions s \
             SET started_at_utc=coalesce(t.source_start,s.started_at_utc,\
             t.first_ts),last_event_at_utc=t.last_ts,active=t.n>0,\
             updated_at=now()
             FROM (\
              SELECT s.id,(\
              SELECT min(c.started_at_utc) \
              FROM canonical_session_sources c \
              WHERE c.session_id=s.id AND c.active) source_start,\
              min(e.timestamp_utc) first_ts,max(e.timestamp_utc) last_ts,\
              count(e.id) n \
              FROM canonical_sessions s \
              LEFT JOIN events e ON e.session_id=s.id AND e.fact_index=0 \
              AND e.active AND e.session_seq IS NOT NULL \
              WHERE s.id IN (\
              SELECT session_id \
              FROM temp.finished_sessions) \
              GROUP BY s.id) t \
              WHERE s.id=t.id;
            -- Native parent UUIDs within a physical stream. Claude API message IDs
            -- group response blocks; outer UUIDs identify individual records.
            INSERT INTO canonical_event_relations \
             SELECT nextval('entity_ids'),child.session_id,child.source_id,\
             child.id,parent.id,child.ordinal::BIGINT,parent.ordinal::BIGINT,\
             'parent','parentUuid','native_metadata',NULL,true,now(),now()
             FROM events child \
              JOIN events parent ON parent.source_id=child.source_id \
              AND parent.session_id=child.session_id \
              AND parent.native_item_id=child.native_parent_id \
              AND parent.id<>child.id AND parent.fact_index=0 \
              AND parent.active AND parent.session_seq IS NOT NULL
             WHERE child.source_id IN (\
              SELECT source_id \
              FROM temp.finished_sources) AND child.fact_index=0 \
              AND child.active AND child.session_seq IS NOT NULL \
              AND child.native_parent_id<>''
             AND NOT EXISTS(\
              SELECT 1 \
              FROM canonical_event_relations r \
              WHERE r.child_event_id=child.id AND r.parent_event_id=parent.id \
              AND r.relation_kind='parent');
            DROP TABLE temp.finished_sessions;
        ",
        )?;
        con.execute_batch(
            "INSERT INTO meta
             SELECT DISTINCT 'project_pending:' || store_id,'' FROM temp.finished_sources
             UNION ALL
             SELECT DISTINCT 'conversation_graph_pending:' || store_id,'' FROM temp.finished_sources
             ON CONFLICT DO NOTHING;
             DROP TABLE temp.finished_sources;",
        )?;
        Ok(())
    }

    pub(crate) fn refresh_conversations(&self) -> Result<()> {
        self.database.connection().execute_batch(
            "UPDATE canonical_session_relations r
             SET target_session_id=NULL,updated_at=now()
             FROM canonical_sessions p
             WHERE p.id=r.target_session_id AND NOT p.active
             AND r.source_session_id IN (SELECT id FROM temp.graph_sessions);
             UPDATE canonical_session_relations r
             SET active=false,updated_at=now()
             FROM canonical_sessions s
             WHERE s.id=r.source_session_id AND NOT s.active AND r.active
             AND s.id IN (SELECT id FROM temp.graph_sessions);
             UPDATE canonical_context_epochs ep
             SET active=false
             WHERE ep.active AND ep.session_id IN (SELECT id FROM temp.graph_sessions) AND NOT EXISTS (
                 SELECT 1 FROM events e
                 WHERE e.context_epoch_id=ep.id AND e.fact_index=0 AND e.active AND e.session_seq IS NOT NULL
             );",
        )?;
        self.database.connection().execute_batch(
            "UPDATE canonical_session_relations r \
             SET target_session_id=p.id,updated_at=now() \
             FROM canonical_sessions c,canonical_sessions p \
             WHERE c.id=r.source_session_id AND c.store_id=p.store_id \
             AND c.harness_key=p.harness_key \
             AND p.native_session_id=r.target_native_session_id AND r.active \
             AND p.active AND r.target_session_id IS DISTINCT FROM p.id \
             AND c.id IN (SELECT id FROM temp.graph_sessions)",
        )?;
        let mut parents = BTreeMap::new();
        let mut st = self
            .database
            .connection()
            .prepare("SELECT id FROM canonical_sessions WHERE active AND id IN (SELECT id FROM temp.graph_sessions) ORDER BY id")?;
        for id in st.query_map([], |r| r.get::<_, i64>(0))? {
            let id = id?;
            parents.insert(id, id);
        }
        let mut edges = Vec::new();
        let mut st = self.database.connection().prepare(
            "SELECT source_session_id,target_session_id \
             FROM canonical_session_relations \
             WHERE active AND target_session_id IS NOT NULL \
             AND source_session_id IN (SELECT id FROM temp.graph_sessions) \
             ORDER BY id",
        )?;
        for pair in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (child, parent) = pair?;
            if parents.contains_key(&child) && parents.contains_key(&parent) {
                let a = root(&mut parents, child);
                let b = root(&mut parents, parent);
                parents.insert(a, b);
                edges.push((child, parent));
            }
        }
        let mut groups: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        let session_ids = parents.keys().copied().collect::<Vec<_>>();
        for id in session_ids {
            groups.entry(root(&mut parents, id)).or_default().push(id);
        }
        let children: std::collections::HashSet<_> =
            edges.iter().map(|(child, _)| *child).collect();
        let (mut roots, mut sessions, mut kinds) = (Vec::new(), Vec::new(), Vec::new());
        for members in groups.values() {
            let root = members
                .iter()
                .find(|id| !children.contains(id))
                .unwrap_or(&members[0]);
            for member in members {
                roots.push(duckdb::types::Value::BigInt(*root));
                sessions.push(duckdb::types::Value::BigInt(*member));
                kinds.push(duckdb::types::Value::Text(
                    if member == root { "root" } else { "related" }.into(),
                ));
            }
        }
        self.database.connection().execute(
            "CREATE TEMP TABLE _conversation_memberships AS \
             SELECT unnest(?::BIGINT[]) root_session_id,\
             unnest(?::BIGINT[]) session_id,\
             unnest(?::VARCHAR[]) membership_kind",
            params![
                duckdb::types::Value::List(roots),
                duckdb::types::Value::List(sessions),
                duckdb::types::Value::List(kinds)
            ],
        )?;
        self.database.connection().execute_batch("
            INSERT INTO canonical_conversation_groups
              SELECT DISTINCT root_session_id,root_session_id,\
               'native-relations-v1',true,now(),now() \
               FROM _conversation_memberships
              ON CONFLICT(root_session_id) \
               DO UPDATE SET active=true,updated_at=now() \
               WHERE NOT canonical_conversation_groups.active;
            ALTER TABLE _conversation_memberships ADD COLUMN conversation_group_id BIGINT;
            UPDATE _conversation_memberships m \
             SET conversation_group_id=g.id \
             FROM canonical_conversation_groups g \
             WHERE g.root_session_id=m.root_session_id;
            DELETE FROM canonical_conversation_memberships m \
             WHERE m.session_id IN (SELECT id FROM temp.graph_sessions) AND NOT EXISTS(\
             SELECT 1 \
             FROM _conversation_memberships n \
             WHERE n.conversation_group_id=m.conversation_group_id \
             AND n.session_id=m.session_id);
            INSERT INTO canonical_conversation_memberships
              SELECT conversation_group_id,session_id,membership_kind,\
               'resolved_relation',true,now() \
               FROM _conversation_memberships
              ON CONFLICT(conversation_group_id,session_id) \
               DO UPDATE SET membership_kind=excluded.membership_kind,\
               evidence_kind=excluded.evidence_kind,is_primary=true,\
               updated_at=now()
              WHERE canonical_conversation_memberships.membership_kind IS DISTINCT FROM excluded.membership_kind \
               OR canonical_conversation_memberships.evidence_kind IS DISTINCT FROM excluded.evidence_kind \
               OR NOT canonical_conversation_memberships.is_primary;
            UPDATE canonical_conversation_groups g \
             SET active=false,updated_at=now() \
             WHERE g.active AND g.root_session_id IN (SELECT id FROM temp.graph_sessions) AND NOT EXISTS(\
             SELECT 1 \
             FROM _conversation_memberships m \
             WHERE m.conversation_group_id=g.id);
            DROP TABLE _conversation_memberships;
        ")?;
        Ok(())
    }

    pub(crate) fn sync_canonical_projects(&self, profile: &Profile) -> Result<()> {
        self.database.connection().execute(
            "INSERT INTO canonical_projects \
             SELECT id,NULL,profile_id,?,native_id,kind,\
             CASE WHEN kind='cwd' THEN NULL ELSE name END,name,\
             CASE WHEN kind='cwd' THEN 'cwd_basename' ELSE 'native_name' END,\
             metadata,active,now(),now() \
             FROM projects \
             WHERE profile_id=? ON CONFLICT(id) \
             DO UPDATE SET native_name=excluded.native_name,\
             display_name=excluded.display_name,\
             metadata_json=excluded.metadata_json,active=excluded.active,\
             updated_at=now() \
             WHERE canonical_projects.display_name IS DISTINCT FROM excluded.display_name \
             OR canonical_projects.metadata_json IS DISTINCT FROM excluded.metadata_json \
             OR canonical_projects.active IS DISTINCT FROM excluded.active",
            params![profile.harness.key(), profile.id],
        )?;
        self.database.connection().execute(
            "UPDATE canonical_project_roots \
             SET active=false \
             WHERE active AND project_id IN (\
             SELECT id \
             FROM projects \
             WHERE profile_id=?) AND NOT EXISTS(\
             SELECT 1 \
             FROM project_roots r \
             WHERE r.project_id=canonical_project_roots.project_id \
             AND r.path=canonical_project_roots.root_path)",
            [&profile.id],
        )?;
        self.database.connection().execute(
            "INSERT INTO canonical_project_roots \
             SELECT r.project_id,r.path,r.ordinal,\
             CASE WHEN p.kind='cwd' THEN 'working_directory' ELSE 'workspace_root' END,\
             CASE WHEN p.kind='cwd' THEN 'cwd_fallback' ELSE 'native_project_root' END,\
             p.active,now(),now() \
             FROM project_roots r \
             JOIN projects p ON p.id=r.project_id \
             WHERE p.profile_id=? ON CONFLICT(project_id,root_path) \
             DO UPDATE SET root_ordinal=excluded.root_ordinal,\
             active=excluded.active \
             WHERE canonical_project_roots.root_ordinal IS DISTINCT FROM excluded.root_ordinal \
             OR canonical_project_roots.active IS DISTINCT FROM excluded.active",
            [&profile.id],
        )?;
        self.database.connection().execute(
            "UPDATE canonical_session_projects sp \
             SET active=false \
             WHERE sp.session_id IN (\
             SELECT id \
             FROM sessions \
             WHERE profile_id=?) AND sp.active \
             AND (sp.evidence_kind IN ('native_root','root_path_match','cwd',\
             'cwd_fallback') AND NOT EXISTS(\
             SELECT 1 \
             FROM project_assignments a \
             WHERE a.session_id=sp.session_id \
             AND a.project_id=sp.project_id) OR NOT EXISTS(\
             SELECT 1 \
             FROM canonical_projects p \
             WHERE p.id=sp.project_id AND p.active))",
            [&profile.id],
        )?;
        self.database.connection().execute("INSERT INTO canonical_session_projects \
                          SELECT a.session_id,a.project_id,NULL,\
                          CASE a.evidence WHEN 'native_root' THEN 'root_path_match' WHEN 'cwd' THEN 'cwd_fallback' ELSE a.evidence END,\
                          a.basis_cwd,true,now(),now() \
                          FROM project_assignments a \
                          JOIN sessions s ON s.id=a.session_id \
                          WHERE s.profile_id=? ON CONFLICT(session_id,project_id) \
                          DO UPDATE SET evidence_kind=excluded.evidence_kind,\
                          basis_working_directory=excluded.basis_working_directory,\
                          active=true \
                          WHERE NOT canonical_session_projects.active \
                          OR canonical_session_projects.evidence_kind IS DISTINCT FROM excluded.evidence_kind \
                          OR canonical_session_projects.basis_working_directory IS DISTINCT FROM excluded.basis_working_directory",[&profile.id])?;
        self.database.connection().execute(
            "UPDATE canonical_session_projects a \
             SET active=false,updated_at=now() \
             FROM sessions s,canonical_projects p \
             WHERE s.id=a.session_id AND p.id=a.project_id \
             AND s.profile_id=? AND a.active \
             AND a.evidence_kind='native_explicit' AND NOT EXISTS(\
             SELECT 1 \
             FROM native_project_assignments n \
             WHERE n.profile_id=s.profile_id \
             AND n.session_native=s.native_id \
             AND n.project_native=p.native_project_id)",
            [&profile.id],
        )?;
        self.database.connection().execute(
            "INSERT INTO canonical_session_projects \
             SELECT s.id,p.id,NULL,'native_explicit',s.cwd,true,now(),now() \
             FROM native_project_assignments n \
             JOIN sessions s ON s.profile_id=n.profile_id \
             AND s.native_id=n.session_native \
             JOIN projects p ON p.profile_id=n.profile_id \
             AND p.native_id=n.project_native AND p.active \
             WHERE n.profile_id=? ON CONFLICT(session_id,project_id) \
             DO UPDATE SET evidence_kind='native_explicit',active=true \
             WHERE NOT canonical_session_projects.active \
             OR canonical_session_projects.evidence_kind<>'native_explicit'",
            [&profile.id],
        )?;
        Ok(())
    }
}
fn root(parents: &mut BTreeMap<i64, i64>, mut id: i64) -> i64 {
    // Union only connects roots. Halve the path so ordered fork chains
    // cannot make all subsequent lookups walk the entire chain again.
    while parents[&id] != id {
        let grandparent = parents[&parents[&id]];
        parents.insert(id, grandparent);
        id = grandparent;
    }
    id
}

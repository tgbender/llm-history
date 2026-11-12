use super::*;
use duckdb::types::Value as SqlValue;
use llm_history_types::query::{SessionSummary, SessionTurns};

pub(crate) struct Conditions {
    pub sql: Vec<String>,
    pub args: Vec<SqlValue>,
}
impl Conditions {
    pub fn new() -> Self {
        Self {
            sql: Vec::new(),
            args: Vec::new(),
        }
    }
    pub fn add(&mut self, sql: &str, value: impl Into<SqlValue>) {
        self.sql.push(sql.into());
        self.args.push(value.into());
    }
    pub fn clause(&self) -> String {
        self.sql.join(" AND ")
    }
    pub fn profiles(
        &mut self,
        store: &Store,
        names: &[String],
        source: Option<&str>,
    ) -> Result<()> {
        if !names.is_empty() {
            store.profiles(names, false)?;
            self.add(
                "list_contains(?::VARCHAR[],p.name)",
                SqlValue::List(names.iter().cloned().map(SqlValue::Text).collect()),
            );
        }
        if let Some(source) = source.filter(|s| *s != "all") {
            let harness: Harness = source.parse()?;
            let ids = store
                .profiles(names, false)?
                .into_iter()
                .filter(|p| p.harness == harness)
                .map(|p| SqlValue::Text(p.id))
                .collect();
            self.add("list_contains(?::VARCHAR[],p.id)", SqlValue::List(ids));
        }
        Ok(())
    }
    /// Working-directory and project conditions on the session aliased `s`.
    pub fn location(&mut self, f: &Filter) {
        if let Some(cwd) = &f.cwd {
            self.add("s.cwd=?", cwd.clone());
        }
        if let Some(root) = &f.under {
            let root = root.replace('\\', "/");
            let root = root.trim_end_matches('/');
            self.sql.push(
                "(replace(s.cwd,chr(92),'/')=? OR starts_with(replace(s.cwd,chr(92),'/'),?))"
                    .into(),
            );
            self.args.push(root.to_owned().into());
            self.args.push(format!("{root}/").into());
        }
        if let Some(project) = &f.project {
            self.sql.push(
                "EXISTS(\
                 SELECT 1 \
                 FROM canonical_session_projects a \
                 JOIN canonical_projects pr ON pr.id=a.project_id \
                 WHERE a.session_id=s.id AND pr.store_id=s.profile_id \
                 AND a.active AND pr.active AND (lower(pr.display_name)=lower(?) \
                 OR pr.native_project_id=?))"
                    .into(),
            );
            self.args
                .extend([project.clone().into(), project.clone().into()]);
        }
    }
}

impl Store {
    pub(crate) fn harness_sql(&self) -> Result<String> {
        let profiles = self.profiles(&[], false)?;
        if profiles.is_empty() {
            return Ok("''".into());
        }
        Ok(format!(
            "CASE p.id {} ELSE '' END",
            profiles
                .iter()
                .map(|p| format!(
                    "WHEN '{}' THEN '{}'",
                    p.id.replace('\'', "''"),
                    p.harness.key()
                ))
                .collect::<Vec<_>>()
                .join(" ")
        ))
    }

    pub(crate) fn message_conditions(&self, f: &Filter) -> Result<Conditions> {
        let mut q = Conditions::new();
        q.sql.push(format!(
            "e.active AND e.kind='message' AND e.text<>'' AND {MESSAGE_RECORD}"
        ));
        q.profiles(self, &f.profiles, f.source.as_deref())?;
        if let Some(role) = &f.role {
            q.add("e.role=?", role.clone());
        }
        if !f.threads.is_empty() {
            q.add(
                "list_contains(?::VARCHAR[],s.native_id)",
                SqlValue::List(f.threads.iter().cloned().map(SqlValue::Text).collect()),
            );
        }
        if llm_history_core::visibility::hides_subagents(f) {
            q.sql.push("NOT s.subagent".into());
        }
        if let Some(kind) = &f.kind {
            q.add("cs.session_kind=?", kind.clone());
        }
        if llm_history_core::visibility::hides_agent_protocol(f) {
            q.sql.push(
                "NOT (history_authorship(e.excluded,e.role,\
                 cs.session_kind)='agent' AND e.role='user') \
                 AND NOT coalesce(e.agent_protocol,false)"
                    .into(),
            );
        }
        q.location(f);
        for (bound, op) in [(&f.since, ">="), (&f.until, "<=")] {
            if let Some(bound) = bound {
                chrono::DateTime::parse_from_rfc3339(bound)?;
                q.add(
                    &format!("e.timestamp_utc{op}cast(? AS TIMESTAMPTZ)"),
                    bound.clone(),
                );
            }
        }
        Ok(q)
    }

    /// Bounded ID-ordered materialization for application readers.
    pub fn messages_page(
        &self,
        f: &Filter,
        after: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let mut q = self.message_conditions(f)?;
        if let Some(after) = after {
            q.add("e.id>?", after);
        }
        let message_query = message_query(&self.harness_sql()?);
        let mut st = self.database.connection().prepare(&format!(
            "{message_query} WHERE {} ORDER BY e.id LIMIT {}",
            q.clause(),
            limit.min(i64::MAX as usize)
        ))?;
        Ok(st
            .query_map(duckdb::params_from_iter(q.args), message_row)?
            .collect::<duckdb::Result<Vec<_>>>()?)
    }
    pub fn context(
        &self,
        message_no: i64,
        before: usize,
        after: usize,
        profiles: &[String],
    ) -> Result<Vec<Message>> {
        let mut q = Conditions::new();
        q.add(
            &format!(
                "mn.message_no=? AND e.active AND e.kind='message' AND e.text<>'' AND {MESSAGE_RECORD}"
            ),
            message_no,
        );
        q.profiles(self, profiles, None)?;
        let message_query = message_query(&self.harness_sql()?);
        let query = format!(
            "WITH anchor AS (\
             SELECT e.id,e.session_id \
             FROM events e \
             JOIN message_numbers mn ON mn.event_id=e.id \
             JOIN sessions s ON s.id=e.session_id \
             JOIN profiles p ON p.id=s.profile_id \
             WHERE {}),
            ordered AS (\
             SELECT e.id,row_number() OVER (ORDER BY e.session_seq,e.id) n \
             FROM events e \
             WHERE e.session_id=(\
             SELECT session_id \
             FROM anchor) AND e.active AND e.kind='message' AND e.text<>'' \
             AND {MESSAGE_RECORD}),
            chosen AS (\
             SELECT * \
             FROM ordered \
             WHERE n BETWEEN (\
             SELECT n \
             FROM ordered \
             WHERE id=(\
             SELECT id \
             FROM anchor))-? AND (\
             SELECT n \
             FROM ordered \
             WHERE id=(\
             SELECT id \
             FROM anchor))+?)
            {message_query} JOIN chosen c ON c.id=e.id ORDER BY c.n",
            q.clause()
        );
        q.args.extend([
            SqlValue::BigInt(before.min(100) as i64),
            SqlValue::BigInt(after.min(100) as i64),
        ]);
        let result = self
            .database
            .connection()
            .prepare(&query)?
            .query_map(duckdb::params_from_iter(q.args), message_row)?
            .collect::<duckdb::Result<Vec<_>>>()?;
        if result.is_empty() {
            bail!("message not found in selected profiles");
        }
        Ok(result)
    }
    /// A session's user turns in conversation order, and its last final answer.
    pub fn session_turns(&self, session_no: i64, profiles: &[String]) -> Result<SessionTurns> {
        let mut known = Conditions::new();
        known.add("sn.session_no=?", session_no);
        known.profiles(self, profiles, None)?;
        let found: bool = self.database.connection().query_row(
            &format!(
                "SELECT EXISTS(\
                 SELECT 1 \
                 FROM session_numbers sn \
                 JOIN sessions s ON s.id=sn.session_id \
                 JOIN profiles p ON p.id=s.profile_id \
                 WHERE {})",
                known.clause()
            ),
            duckdb::params_from_iter(known.args),
            |r| r.get(0),
        )?;
        if !found {
            bail!("session not found in selected profiles");
        }
        let message_query = message_query(&self.harness_sql()?);
        let messages = |role: &str, order: &str| -> Result<Vec<Message>> {
            // A named session shows its turns whoever wrote them, subagent prompts included.
            let mut q = self.retrieval_conditions(&Filter {
                profiles: profiles.to_vec(),
                role: Some(role.into()),
                include_subagents: true,
                include_agent_protocol: true,
                ..Filter::default()
            })?;
            q.add("sn.session_no=?", session_no);
            Ok(self
                .database
                .connection()
                .prepare(&format!(
                    "{message_query} WHERE {} ORDER BY {order}",
                    q.clause()
                ))?
                .query_map(duckdb::params_from_iter(q.args), message_row)?
                .collect::<duckdb::Result<Vec<_>>>()?)
        };
        Ok(SessionTurns {
            turns: messages("user", "e.session_seq,e.id")?,
            last_answer: messages("assistant", "e.session_seq DESC,e.id DESC LIMIT 1")?.pop(),
        })
    }
    pub fn list_sessions(
        &self,
        f: &Filter,
        limit: usize,
        oldest: bool,
    ) -> Result<Vec<SessionSummary>> {
        let mut q = Conditions::new();
        q.sql.push("true".into());
        q.profiles(self, &f.profiles, f.source.as_deref())?;
        if llm_history_core::visibility::hides_subagents(f) {
            q.sql.push("NOT s.subagent".into());
        }
        if let Some(kind) = &f.kind {
            q.add("cs.session_kind=?", kind.clone());
        }
        self.validate_project_filter(f)?;
        q.location(f);
        let mut outer = Vec::new();
        for (bound, op) in [(&f.since, ">="), (&f.until, "<=")] {
            if let Some(bound) = bound {
                chrono::DateTime::parse_from_rfc3339(bound)?;
                outer.push(format!(
                    "try_cast(created_at AS TIMESTAMPTZ){op}cast(? AS TIMESTAMPTZ)"
                ));
                q.args.push(bound.clone().into());
            }
        }
        let harness = self.harness_sql()?;
        let query = format!(
            "WITH summary AS (
            SELECT s.id,p.name profile,{harness} provider,\
             s.native_id thread_id,s.cwd,coalesce(h.project_name,\
             s.project) project,coalesce(s.title,'') title,coalesce(nullif(t.model,''),s.model,\
             '') model,s.parent_native,s.relation,s.subagent,
            strftime(h.started_at_utc,'%Y-%m-%dT%H:%M:%S.%gZ') created_at,\
             strftime(h.last_event_at_utc,\
             '%Y-%m-%dT%H:%M:%S.%gZ') updated_at,
            coalesce(nullif(t.reasoning_effort,''),(\
             SELECT arg_max(nullif(e.effort,''),e.id) \
             FROM events e \
             WHERE e.session_id=s.id AND e.active),'') effort,\
             h.message_count messages,
            h.conversation_group_id,h.native_runtime_id,h.session_kind,\
             h.native_session_kind,h.session_kind_evidence,h.agent_label,\
             h.tool_call_count,h.compaction_count,h.child_session_count,\
             cs.metadata_json,sn.session_no
            FROM history_sessions h \
             JOIN sessions s ON s.id=h.session_id \
             JOIN canonical_sessions cs ON cs.id=s.id \
             JOIN profiles p ON p.id=s.profile_id
             JOIN session_numbers sn ON sn.session_id=s.id
            LEFT JOIN native_threads t ON t.profile_id=s.profile_id AND t.id=s.native_id
            WHERE {})
            SELECT id,profile,provider,thread_id,cwd,project,title,model,\
             parent_native,relation,subagent,created_at,updated_at,effort,\
             messages,conversation_group_id,native_runtime_id,session_kind,\
             native_session_kind,session_kind_evidence,agent_label,\
             tool_call_count,compaction_count,child_session_count,\
             metadata_json,session_no \
             FROM summary {} \
             ORDER BY try_cast(created_at AS TIMESTAMPTZ) {} NULLS LAST,id \
             LIMIT ?",
            q.clause(),
            if outer.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", outer.join(" AND "))
            },
            if oldest { "ASC" } else { "DESC" }
        );
        q.args.push(SqlValue::BigInt(limit.min(1000) as i64));
        let mut st = self.database.connection().prepare(&query)?;
        let rows = st.query_map(duckdb::params_from_iter(q.args), |r| {
            let metadata: Value = r
                .get::<_, Option<String>>(24)?
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let provider: String = r.get(2)?;
            let subagent: bool = r.get(10)?;
            let session_kind: String = r.get(17)?;
            let role = metadata
                .pointer("/source/subagent/thread_spawn/agent_role")
                .or_else(|| metadata.get("agent_role"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let role = if session_kind == "guardian" {
                "guardian"
            } else if role.is_empty() && subagent && provider == "claude" {
                "subagent"
            } else {
                role
            };
            Ok(SessionSummary {
                id: r.get::<_, i64>(0)?.to_string(),
                session_no: r.get(25)?,
                profile: r.get(1)?,
                provider,
                thread_id: r.get(3)?,
                cwd: r.get(4)?,
                project: r.get(5)?,
                title: r.get(6)?,
                model: r.get(7)?,
                parent_thread_id: r.get(8)?,
                relation: r.get(9)?,
                subagent,
                created_at: r.get(11)?,
                updated_at: r.get(12)?,
                effort: r.get(13)?,
                messages: r.get(14)?,
                conversation_group_id: r.get::<_, Option<i64>>(15)?.map(|v| v.to_string()),
                native_runtime_id: r.get(16)?,
                session_kind,
                native_session_kind: r.get(18)?,
                session_kind_evidence: r.get(19)?,
                agent_label: r.get(20)?,
                tool_calls: r.get(21)?,
                compactions: r.get(22)?,
                children: r.get(23)?,
                role: role.into(),
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }
}

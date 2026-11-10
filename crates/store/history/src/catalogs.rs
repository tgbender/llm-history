use super::*;
use duckdb::OptionalExt;
use llm_history_store_harnesses::{catalog as native_catalog, codex::threads as native_threads};
use llm_history_types::catalog::{CatalogProject, ProjectCatalog};
use llm_history_types::query::ProjectSummary;
use std::collections::{BTreeMap, BTreeSet, HashMap};
// Matching a root is equivalent to looking up the path and each slash prefix.
struct RootIndex<T> {
    exact: HashMap<String, (usize, usize, T)>,
    windows: HashMap<String, (usize, usize, T)>,
}
impl<T> Default for RootIndex<T> {
    fn default() -> Self {
        Self {
            exact: HashMap::new(),
            windows: HashMap::new(),
        }
    }
}
impl<T> RootIndex<T> {
    fn insert(&mut self, root: &str, rank: usize, value: T) {
        let normalized = root.replace('\\', "/");
        let (map, key) = if normalized.as_bytes().get(1) == Some(&b':') {
            (&mut self.windows, normalized.to_lowercase())
        } else {
            (&mut self.exact, normalized)
        };
        let key = key.trim_end_matches('/');
        let score = (root.len(), rank);
        if map
            .get(key)
            .is_none_or(|&(length, rank, _)| (length, rank) <= score)
        {
            map.insert(key.to_owned(), (score.0, score.1, value));
        }
    }
    fn best(&self, path: &str) -> Option<&T> {
        fn candidates<'a, T>(
            map: &'a HashMap<String, (usize, usize, T)>,
            path: &str,
        ) -> Option<&'a (usize, usize, T)> {
            std::iter::once(path)
                .chain(path.match_indices('/').map(|(i, _)| &path[..i]))
                .filter_map(|key| map.get(key))
                .max_by_key(|&(length, rank, _)| (*length, *rank))
        }
        let path = path.replace('\\', "/");
        let exact = candidates(&self.exact, &path);
        let folded = (!self.windows.is_empty()).then(|| path.to_lowercase());
        let windows = folded.as_deref().and_then(|p| candidates(&self.windows, p));
        exact
            .into_iter()
            .chain(windows)
            .max_by_key(|&(length, rank, _)| (*length, *rank))
            .map(|row| &row.2)
    }
    fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.windows.is_empty()
    }
}

impl Store {
    pub fn validate_project_filter(&self, filter: &Filter) -> Result<()> {
        let Some(project) = filter.project.as_ref().filter(|s| !s.is_empty()) else {
            return Ok(());
        };
        let mut conditions = super::queries::Conditions::new();
        conditions.sql.push("pr.active".into());
        conditions.profiles(self, &filter.profiles, filter.source.as_deref())?;
        conditions
            .sql
            .push("(lower(pr.display_name)=lower(?) OR pr.native_project_id=?)".into());
        conditions
            .args
            .extend([project.clone().into(), project.clone().into()]);
        let query = format!(
            "SELECT pr.id,pr.display_name,pr.native_project_id,{},\
             r.root_path \
             FROM canonical_projects pr \
             JOIN profiles p ON p.id=pr.store_id \
             LEFT JOIN canonical_project_roots r ON r.project_id=pr.id \
             AND r.active \
             WHERE {} \
             ORDER BY pr.id,r.root_path",
            self.harness_sql()?,
            conditions.clause()
        );
        let mut statement = self.database.connection().prepare(&query)?;
        let mut rows = statement.query(duckdb::params_from_iter(conditions.args))?;
        let mut matches = std::collections::BTreeMap::<
            i64,
            (String, String, String, std::collections::BTreeSet<String>),
        >::new();
        while let Some(row) = rows.next()? {
            let entry = matches.entry(row.get(0)?).or_insert((
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                Default::default(),
            ));
            if let Some(root) = row.get::<_, Option<String>>(4)? {
                entry.3.insert(root);
            }
        }
        if matches.is_empty() {
            bail!(
                "Unknown project {project:?} in selected profiles/source; run 'llm-history projects list' to discover names"
            );
        }
        let locations = matches
            .values()
            .filter(|p| !p.3.is_empty())
            .map(|p| p.3.clone())
            .collect::<std::collections::BTreeSet<_>>();
        if locations.len() > 1 && !matches.values().any(|p| p.1 == *project) {
            let choices = matches
                .values()
                .map(|p| {
                    format!(
                        "{} ({}: {})",
                        p.0,
                        p.2,
                        p.3.iter().cloned().collect::<Vec<_>>().join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "Project name {project:?} is ambiguous: {choices}; use an exact project ID, --cwd or --under"
            );
        }
        Ok(())
    }

    fn upsert_projects<'a>(
        &self,
        profile: &Profile,
        projects: impl IntoIterator<Item = &'a CatalogProject>,
    ) -> Result<()> {
        let projects: BTreeMap<_, _> = projects.into_iter().map(|p| (&p.native_id, p)).collect();
        if projects.is_empty() {
            return Ok(());
        }
        let con = self.database.connection();
        con.execute_batch(
            "CREATE TEMP TABLE project_input(profile_id VARCHAR,native_id VARCHAR,name VARCHAR,kind VARCHAR,metadata VARCHAR);
             CREATE TEMP TABLE project_root_input(native_id VARCHAR,path VARCHAR,ordinal INTEGER);",
        )?;
        {
            let mut project = con.appender_to_catalog_and_db("project_input", "temp", "main")?;
            let mut roots = con.appender_to_catalog_and_db("project_root_input", "temp", "main")?;
            for p in projects.into_values() {
                project.append_row(params![
                    profile.id,
                    p.native_id,
                    p.name,
                    p.kind,
                    serde_json::to_string(&p.metadata)?
                ])?;
                let mut seen = BTreeSet::new();
                for (ordinal, root) in p.roots.iter().enumerate() {
                    if seen.insert(root) {
                        roots.append_row(params![p.native_id, root, ordinal as i32])?;
                    }
                }
            }
            project.flush()?;
            roots.flush()?;
        }
        con.execute_batch(include_str!("project_batch.sql"))?;
        Ok(())
    }

    pub fn catalog_revision(&self, profile: &Profile) -> Result<Option<String>> {
        Ok(self
            .database
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key=?",
                [format!("catalog:{}", profile.id)],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn publish_catalog(
        &self,
        profile: &Profile,
        input: llm_history_types::ingestion::PreparedCatalog,
    ) -> Result<CatalogReceipt> {
        let key = format!("catalog:{}", profile.id);
        let pending = format!("project_pending:{}", profile.id);
        let llm_history_types::ingestion::PreparedCatalog {
            previous,
            fingerprint,
            loaded,
            threads,
        } = input;
        let con = self.database.connection();
        anyhow::ensure!(
            self.catalog_revision(profile)? == previous,
            "catalog changed during preparation"
        );
        if loaded.is_none()
            && threads.is_none()
            && previous.as_deref() == Some(&fingerprint)
            && !con.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?)",
                [&pending],
                |r| r.get(0),
            )?
        {
            return Ok(CatalogReceipt::default());
        }
        let mut result = CatalogReceipt::default();
        let transaction = self.database.begin()?;
        let applied = (|| -> Result<()> {
            if let Some(rows) = threads
                && native_threads::write(&transaction, &profile.id, &rows)? > 0
            {
                self.sync_thread_metadata()?;
            }
            if let Some((path, format, catalog)) = loaded {
                let data = serde_json::to_string(&catalog)?;
                if data.len() > 32 * 1024 * 1024 {
                    bail!("logical catalog exceeds byte budget");
                }
                let previous = native_catalog::current(self.database.connection(), &profile.id)?;
                if previous.as_ref().map(|snapshot| snapshot.data.as_str()) != Some(data.as_str()) {
                    let old_catalog: ProjectCatalog = previous
                        .as_ref()
                        .map(|snapshot| serde_json::from_str(&snapshot.data))
                        .transpose()?
                        .unwrap_or_default();
                    let old: BTreeMap<_, _> = old_catalog
                        .projects
                        .iter()
                        .map(|p| (&p.native_id, &p.roots))
                        .collect();
                    let new: BTreeMap<_, _> = catalog
                        .projects
                        .iter()
                        .map(|p| (&p.native_id, &p.roots))
                        .collect();
                    let mut changed_roots = RootIndex::default();
                    for (from, to) in [(&old, &new), (&new, &old)] {
                        for (&key, &roots) in from {
                            if to.get(key).copied() != Some(roots) {
                                for root in roots {
                                    changed_roots.insert(root, 0, ());
                                }
                            }
                        }
                    }
                    let snapshot = match previous {
                        Some(snapshot) => snapshot.id,
                        None => self.next_id()?,
                    };
                    let digest = blake3::hash(data.as_bytes()).to_hex().to_string();
                    native_catalog::write_snapshot(
                        &transaction,
                        native_catalog::SnapshotWrite {
                            id: snapshot,
                            profile: &profile.id,
                            uri: &path.to_string_lossy(),
                            format,
                            digest: &digest,
                            data: &data,
                        },
                    )?;
                    let native_ids = duckdb::types::Value::List(
                        catalog
                            .projects
                            .iter()
                            .map(|p| duckdb::types::Value::Text(p.native_id.clone()))
                            .collect(),
                    );
                    self.database.connection().execute(
                        "update projects \
                             set active=false \
                             where profile_id=? and kind<>'cwd' and active and native_id not in (\
                             select unnest(?::VARCHAR[]))",
                        params![profile.id, native_ids],
                    )?;
                    self.upsert_projects(profile, &catalog.projects)?;
                    result.projects += catalog.projects.len();
                    let new_assignments = catalog
                        .assignments
                        .iter()
                        .map(|a| (a.session.clone(), a.project.clone()))
                        .collect();
                    let changed_sessions = native_catalog::reconcile_assignments(
                        &transaction,
                        &profile.id,
                        &new_assignments,
                    )?;
                    if !changed_sessions.is_empty() {
                        let ids = duckdb::types::Value::List(
                            changed_sessions
                                .into_iter()
                                .map(duckdb::types::Value::Text)
                                .collect(),
                        );
                        con.execute(
                            "DELETE FROM project_assignments WHERE session_id IN (
                                SELECT id FROM sessions WHERE profile_id=? AND native_id IN (SELECT unnest(?::VARCHAR[]))
                             )", params![profile.id,ids],
                        )?;
                    }
                    if !changed_roots.is_empty() {
                        let mut st =
                            con.prepare("SELECT id,cwd FROM sessions WHERE profile_id=?")?;
                        let mut ids = Vec::new();
                        for row in st.query_map([&profile.id], |r| {
                            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                        })? {
                            let (id, cwd) = row?;
                            if changed_roots.best(&cwd).is_some() {
                                ids.push(id);
                            }
                        }
                        if !ids.is_empty() {
                            con.execute(
                                "DELETE FROM project_assignments WHERE session_id IN (SELECT unnest(?::BIGINT[]))",
                                [embedding_batch::id_list(&ids)],
                            )?;
                        }
                    }
                    result.snapshots += 1;
                }
            }
            self.database.connection().execute(
                "insert into meta \
                     values (?,?) on conflict(key) \
                     do update set value=excluded.value \
                     where meta.value<>excluded.value",
                params![key, fingerprint],
            )?;
            result.assignments += self.reconcile_projects(profile)?;
            con.execute("DELETE FROM meta WHERE key=?", [&pending])?;
            Ok(())
        })();
        match applied {
            Ok(()) => transaction.commit()?,
            Err(error) => {
                return Err(error);
            }
        }
        Ok(result)
    }
    pub fn reconcile_pending_projects<'a>(
        &self,
        profiles: impl IntoIterator<Item = &'a Profile>,
    ) -> Result<()> {
        let tx = self.database.begin()?;
        let con = tx.connection();
        for profile in profiles {
            let pending = format!("project_pending:{}", profile.id);
            if con.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?)",
                [&pending],
                |r| r.get(0),
            )? {
                self.reconcile_projects(profile)?;
                con.execute("DELETE FROM meta WHERE key=?", [&pending])?;
            }
        }
        tx.commit()
    }

    pub(crate) fn reconcile_projects(&self, profile: &Profile) -> Result<usize> {
        let con = self.database.connection();
        let snapshot=con.query_row(
            "SELECT id FROM catalog_snapshots WHERE profile_id=? AND current ORDER BY id DESC LIMIT 1",
            [&profile.id], |r| r.get::<_,i64>(0),
        ).optional()?;
        let sessions = {
            let mut st=con.prepare(
                "WITH candidates AS (
                    SELECT s.id,s.native_id,s.cwd,s.project,a.session_id assigned
                    FROM sessions s LEFT JOIN project_assignments a ON a.session_id=s.id
                    LEFT JOIN projects p ON p.id=a.project_id
                    WHERE s.profile_id=? AND (a.session_id IS NULL OR NOT p.active OR s.project<>p.name OR s.cwd<>a.basis_cwd)
                 ), explicit AS (
                    SELECT s.id,min(p.id) project_id FROM candidates s
                    JOIN native_project_assignments a ON a.profile_id=? AND a.session_native=s.native_id
                    JOIN projects p ON p.profile_id=a.profile_id AND p.native_id=a.project_native AND p.active
                    GROUP BY s.id
                 )
                 SELECT s.id,s.cwd,p.id,p.name FROM candidates s
                 LEFT JOIN explicit e USING (id) LEFT JOIN projects p ON p.id=e.project_id
                 WHERE s.cwd<>'' OR s.assigned IS NOT NULL OR s.project<>'' OR p.id IS NOT NULL
                 ORDER BY s.id",
            )?;
            st.query_map(params![profile.id, profile.id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?
        };
        if sessions.is_empty() {
            self.sync_canonical_projects(profile)?;
            return Ok(0);
        }
        let mut roots = RootIndex::default();
        {
            let mut st=con.prepare(
                "SELECT p.id,p.name,r.path FROM projects p JOIN project_roots r ON r.project_id=p.id
                 WHERE p.profile_id=? AND p.active AND p.kind<>'cwd' ORDER BY p.native_id,r.ordinal",
            )?;
            for (rank, row) in st
                .query_map([&profile.id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .enumerate()
            {
                let (id, name, path) = row?;
                roots.insert(&path, rank, (id, name));
            }
        }
        let mut fallback = BTreeMap::new();
        con.execute_batch(
            "CREATE TEMP TABLE chosen_projects(session_id BIGINT,cwd VARCHAR,project_id BIGINT,name VARCHAR,evidence VARCHAR,native_id VARCHAR,profile_id VARCHAR,snapshot_id BIGINT)",
        )?;
        {
            let mut app = con.appender_to_catalog_and_db("chosen_projects", "temp", "main")?;
            for (session, cwd, id, name) in &sessions {
                let (id, name, evidence, native) = if let Some(id) = id {
                    (
                        Some(*id),
                        name.as_deref().unwrap_or(""),
                        "native_explicit",
                        None,
                    )
                } else if let Some((id, name)) = roots.best(cwd) {
                    (Some(*id), name.as_str(), "native_root", None)
                } else if !cwd.is_empty() {
                    let p = fallback.entry(cwd).or_insert_with(|| CatalogProject {
                        native_id: format!("cwd:{cwd}"),
                        name: project_name(cwd),
                        kind: "cwd".into(),
                        roots: vec![cwd.clone()],
                        metadata: json!({}),
                    });
                    (None, p.name.as_str(), "cwd", Some(p.native_id.as_str()))
                } else {
                    (None, "", "", None)
                };
                app.append_row(params![
                    session, cwd, id, name, evidence, native, profile.id, snapshot
                ])?;
            }
            app.flush()?;
        }
        self.upsert_projects(profile, fallback.values())?;
        con.execute_batch(
            "UPDATE temp.chosen_projects c SET project_id=p.id
             FROM projects p WHERE p.profile_id=c.profile_id AND p.native_id=c.native_id;
             DELETE FROM project_assignments a USING temp.chosen_projects c
             WHERE a.session_id=c.session_id AND c.project_id IS NULL;
             INSERT INTO project_assignments
             SELECT session_id,project_id,evidence,snapshot_id,cwd
             FROM temp.chosen_projects WHERE project_id IS NOT NULL
             ON CONFLICT(session_id) DO UPDATE SET project_id=excluded.project_id,evidence=excluded.evidence,
                 snapshot_id=excluded.snapshot_id,basis_cwd=excluded.basis_cwd;
             UPDATE sessions s SET project=c.name FROM temp.chosen_projects c
             WHERE s.id=c.session_id AND s.project IS DISTINCT FROM c.name;
             DROP TABLE temp.chosen_projects;",
        )?;
        self.database.connection().execute(
            "update projects p \
             set active=false \
             where p.profile_id=? and p.kind='cwd' and not exists(\
             select 1 \
             from project_assignments a \
             join sessions s on s.id=a.session_id \
             where a.project_id=p.id and exists(\
             select 1 \
             from events e \
             where e.session_id=s.id and e.active))",
            [&profile.id],
        )?;
        self.sync_canonical_projects(profile)?;
        Ok(sessions.len())
    }
}

impl Store {
    pub fn list_projects_including(
        &self,
        filter: &Filter,
        limit: usize,
        include_inactive: bool,
    ) -> Result<Vec<ProjectSummary>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let profiles = self
            .profiles(&filter.profiles, false)?
            .into_iter()
            .filter(|p| {
                filter
                    .source
                    .as_deref()
                    .is_none_or(|source| source == "all" || source == p.harness.key())
            })
            .map(|p| (p.id.clone(), p))
            .collect::<HashMap<_, _>>();
        let selected = duckdb::types::Value::List(
            profiles
                .keys()
                .cloned()
                .map(duckdb::types::Value::Text)
                .collect(),
        );
        let mut st = self.database.connection().prepare(
            "WITH page AS MATERIALIZED (
                SELECT p.* FROM projects p WHERE p.profile_id IN (SELECT unnest(?::VARCHAR[]))
                AND (p.active OR ?) AND (? OR p.kind<>'cwd' OR EXISTS (
                    SELECT 1 FROM canonical_session_projects a WHERE a.project_id=p.id AND a.active
                ))
                ORDER BY p.active DESC,lower(p.name),p.id LIMIT ?
             ), roots AS (
                SELECT r.project_id,list(r.path ORDER BY r.ordinal) roots FROM project_roots r
                SEMI JOIN page p ON p.id=r.project_id GROUP BY r.project_id
             ), counts AS (
                SELECT p.id,count(*) n FROM page p JOIN canonical_session_projects a ON a.project_id=p.id AND a.active
                JOIN sessions s ON s.id=a.session_id AND s.profile_id=p.profile_id
                WHERE EXISTS(SELECT 1 FROM events e WHERE e.session_id=s.id AND e.active)
                GROUP BY p.id
             )
             SELECT p.id,p.profile_id,p.native_id,p.name,p.kind,p.metadata,coalesce(c.n,0),p.active,coalesce(r.roots,[]::VARCHAR[])
             FROM page p LEFT JOIN roots r ON r.project_id=p.id LEFT JOIN counts c ON c.id=p.id
             ORDER BY p.active DESC,lower(p.name),p.id",
        )?;
        let mut rows = st.query(params![
            selected,
            include_inactive,
            include_inactive,
            limit.min(i64::MAX as usize) as i64
        ])?;
        let mut output = Vec::new();
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let profile_id: String = row.get(1)?;
            let profile = &profiles[&profile_id];
            let duckdb::types::Value::List(roots) = row.get(8)? else {
                bail!("project roots are not a list")
            };
            let roots = roots
                .into_iter()
                .map(|root| match root {
                    duckdb::types::Value::Text(path) => Ok(path),
                    _ => bail!("project root is not text"),
                })
                .collect::<Result<Vec<_>>>()?;
            output.push(ProjectSummary {
                id: id.to_string(),
                profile: profile.name.clone(),
                source: profile.harness.key().into(),
                native_id: row.get(2)?,
                name: row.get(3)?,
                kind: row.get(4)?,
                metadata: serde_json::from_str(&row.get::<_, String>(5)?)?,
                sessions: row.get(6)?,
                roots,
                active: row.get(7)?,
            });
        }
        Ok(output)
    }
}

impl Store {
    fn sync_thread_metadata(&self) -> Result<()> {
        self.database.connection().execute_batch(
            "UPDATE sessions s \
             SET title=coalesce(nullif(t.label,''),s.title),\
             model=coalesce(nullif(s.model,''),t.model) \
             FROM (\
             SELECT *,thread_title(name,title,preview,first_user_message,agent_path,source) label \
             FROM native_threads) t \
             WHERE t.profile_id=s.profile_id AND t.id=s.native_id \
             AND ((t.label<>'' AND s.title IS DISTINCT FROM t.label) OR s.model='');\
             UPDATE canonical_sessions s \
             SET model=t.model \
             FROM native_threads t \
             WHERE t.profile_id=s.store_id AND t.id=s.native_session_id \
             AND (s.model IS NULL OR s.model='') AND t.model<>''",
        )?;
        Ok(())
    }
}

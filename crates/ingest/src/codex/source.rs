//! Codex SQLite metadata acquisition; never writes source databases.
use anyhow::{Result, bail};
use llm_history_core::project_name;
use llm_history_types::{
    catalog::{CatalogAssignment, CatalogProject, ProjectCatalog},
    native::codex::ThreadMetadata,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct ReadLimits {
    rows: usize,
    bytes: usize,
    time: Duration,
}
const READ_LIMITS: ReadLimits = ReadLimits {
    rows: 100_000,
    bytes: 32 * 1024 * 1024,
    time: Duration::from_secs(10),
};

fn open_source(path: &Path, limits: ReadLimits, deadline: Instant) -> Result<rusqlite::Connection> {
    let con = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    con.busy_timeout(Duration::from_millis(100))?;
    con.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        limits.bytes.try_into()?,
    )?;
    con.progress_handler(10_000, Some(move || Instant::now() >= deadline))?;
    con.execute_batch("PRAGMA query_only=true; BEGIN")?;
    Ok(con)
}
const FIELDS: [&str; 15] = [
    "id",
    "rollout_path",
    "created_at",
    "updated_at",
    "source",
    "cwd",
    "title",
    "archived",
    "first_user_message",
    "model",
    "reasoning_effort",
    "thread_source",
    "preview",
    "name",
    "agent_path",
];

pub fn read_threads(root: &Path) -> Result<Vec<ThreadMetadata>> {
    read_threads_with_limits(root, READ_LIMITS)
}

fn read_threads_with_limits(root: &Path, limits: ReadLimits) -> Result<Vec<ThreadMetadata>> {
    let deadline = Instant::now() + limits.time;
    let mut row_count = 0usize;
    let mut bytes = 0usize;
    let mut latest: BTreeMap<String, (Vec<String>, String)> = BTreeMap::new();
    for path in [
        root.join("state_5.sqlite"),
        root.join("sqlite/state_5.sqlite"),
    ] {
        if !path.is_file() {
            continue;
        }
        if Instant::now() >= deadline {
            bail!("native thread acquisition deadline exceeded");
        }
        let con = open_source(&path, limits, deadline)?;
        let columns = con
            .prepare("PRAGMA table_info(threads)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|c| c == "id") {
            continue;
        }
        let projection = FIELDS
            .iter()
            .map(|field| {
                if columns.iter().any(|c| c == field) {
                    format!("coalesce(cast({field} AS TEXT),'')")
                } else {
                    "''".into()
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut st = con.prepare(&format!(
            "SELECT {projection} FROM threads ORDER BY id LIMIT {}",
            limits.rows + 1
        ))?;
        let mut rows = st.query([])?;
        while let Some(record) = rows.next()? {
            if Instant::now() >= deadline {
                bail!("native thread acquisition deadline exceeded");
            }
            row_count += 1;
            if row_count > limits.rows {
                bail!("native thread limit exceeded");
            }
            bytes = bytes.saturating_add(path.as_os_str().len());
            let mut row = Vec::with_capacity(FIELDS.len());
            for i in 0..FIELDS.len() {
                let value = record.get_ref(i)?.as_str()?;
                bytes = bytes.saturating_add(value.len());
                if bytes > limits.bytes {
                    bail!("native thread byte budget exceeded");
                }
                row.push(value.to_owned());
            }
            if latest.get(&row[0]).is_none_or(|(old, _)| row[3] >= old[3]) {
                latest.insert(row[0].clone(), (row, path.to_string_lossy().into_owned()));
            }
        }
    }
    Ok(latest
        .into_values()
        .map(|(row, source_db)| ThreadMetadata {
            id: row[0].clone(),
            rollout_path: row[1].clone(),
            created_at: row[2].clone(),
            updated_at: row[3].clone(),
            source: row[4].clone(),
            cwd: row[5].clone(),
            title: row[6].clone(),
            archived: matches!(row[7].as_str(), "1" | "true"),
            first_user_message: row[8].clone(),
            model: row[9].clone(),
            reasoning_effort: row[10].clone(),
            thread_source: row[11].clone(),
            preview: row[12].clone(),
            name: row[13].clone(),
            agent_path: row[14].clone(),
            source_db,
        })
        .collect())
}

fn sqlite_pairs(con: &rusqlite::Connection, sql: &str) -> Result<Vec<(String, String)>> {
    let mut statement = con.prepare(sql)?;
    let mut rows = statement.query([])?;
    let mut out = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next()? {
        let left: String = row.get(0)?;
        let right: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        bytes = bytes.saturating_add(left.len()).saturating_add(right.len());
        if out.len() >= READ_LIMITS.rows || bytes > READ_LIMITS.bytes {
            bail!("catalog row/byte budget exceeded");
        }
        out.push((left, right));
    }
    Ok(out)
}
pub fn read_project_catalog(path: &Path) -> Result<Option<ProjectCatalog>> {
    let con = open_source(path, READ_LIMITS, Instant::now() + READ_LIMITS.time)?;
    let tables = con
        .prepare("select name from sqlite_master where type='table'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !["projects", "project_roots", "threads"]
        .iter()
        .all(|t| tables.iter().any(|x| x == t))
    {
        return Ok(None);
    }
    let columns = |table: &str| -> Result<Vec<String>> {
        Ok(con
            .prepare(&format!("pragma table_info('{table}')"))?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    };
    let project_cols = columns("projects")?;
    let root_cols = columns("project_roots")?;
    let thread_cols = columns("threads")?;
    if !["id", "name"]
        .iter()
        .all(|k| project_cols.iter().any(|c| c == k))
        || !["project_id", "path"]
            .iter()
            .all(|k| root_cols.iter().any(|c| c == k))
    {
        return Ok(None);
    }
    let order = if project_cols.iter().any(|c| c == "position") {
        "position,id"
    } else {
        "id"
    };
    let projects = sqlite_pairs(
        &con,
        &format!(
            "select cast(id as text),cast(name as text) from projects order by {order} limit 100001"
        ),
    )?;
    if projects.is_empty() {
        return Ok(None);
    }
    if projects.len() > 100000 {
        bail!("catalog project limit exceeded");
    }
    let order = if root_cols.iter().any(|c| c == "position") {
        "project_id,position,path"
    } else {
        "project_id,path"
    };
    let mut roots = std::collections::HashMap::<String, Vec<String>>::new();
    for (project, root) in sqlite_pairs(
        &con,
        &format!(
            "select cast(project_id as text),cast(path as text) from project_roots order by {order} limit 100001"
        ),
    )? {
        roots.entry(project).or_default().push(root);
    }
    let mut catalog = ProjectCatalog::default();
    for (id, name) in projects {
        let roots = roots.remove(&id).unwrap_or_default();
        if roots.is_empty() {
            continue;
        }
        let name = if name.is_empty() {
            project_name(&roots[0])
        } else {
            name
        };
        catalog.projects.push(CatalogProject {
            native_id: id,
            name,
            kind: "local".into(),
            roots,
            metadata: json!({}),
        });
    }
    if ["id", "project_id"]
        .iter()
        .all(|k| thread_cols.iter().any(|c| c == k))
    {
        let assignments = sqlite_pairs(
            &con,
            "select cast(id as text),cast(project_id as text) \
             from threads \
             where project_id is not null \
             order by id \
             limit 100001",
        )?;
        let known = catalog
            .projects
            .iter()
            .map(|p| p.native_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        catalog.assignments = assignments
            .into_iter()
            .filter(|(_, id)| known.contains(id.as_str()))
            .map(|(session, project)| CatalogAssignment { session, project })
            .collect();
    }
    Ok(Some(catalog))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threads(path: &Path, count: usize, text: &str) -> Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut con = rusqlite::Connection::open(path)?;
        con.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY, first_user_message TEXT)")?;
        let tx = con.transaction()?;
        for n in 0..count {
            tx.execute(
                "INSERT INTO threads VALUES (?,?)",
                rusqlite::params![n.to_string(), text],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    #[test]
    fn thread_row_and_byte_budgets_cover_both_source_databases() -> Result<()> {
        let dir = tempfile::tempdir()?;
        threads(&dir.path().join("state_5.sqlite"), 2, &"x".repeat(600))?;
        threads(
            &dir.path().join("sqlite/state_5.sqlite"),
            2,
            &"x".repeat(600),
        )?;
        // Duplicate native IDs still count as acquired rows and bytes.
        assert_eq!(read_threads(dir.path())?.len(), 2);
        let error = read_threads_with_limits(
            dir.path(),
            ReadLimits {
                rows: 3,
                ..READ_LIMITS
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("thread limit"), "{error:#}");
        let error = read_threads_with_limits(
            dir.path(),
            ReadLimits {
                bytes: 2048,
                ..READ_LIMITS
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("byte budget"), "{error:#}");
        Ok(())
    }

    #[test]
    fn oversized_thread_values_are_rejected_by_sqlite() -> Result<()> {
        let dir = tempfile::tempdir()?;
        threads(&dir.path().join("state_5.sqlite"), 1, &"x".repeat(4096))?;
        let error = read_threads_with_limits(
            dir.path(),
            ReadLimits {
                bytes: 2048,
                ..READ_LIMITS
            },
        )
        .unwrap_err();
        assert!(
            matches!(error.downcast_ref::<rusqlite::Error>(), Some(rusqlite::Error::SqliteFailure(code, _)) if code.code == rusqlite::ErrorCode::TooBig),
            "{error:#}"
        );
        Ok(())
    }

    #[test]
    fn acquisition_deadline_stops_scans_and_sqlite_computation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("state_5.sqlite");
        threads(&path, 1, "message")?;
        let error = read_threads_with_limits(
            dir.path(),
            ReadLimits {
                time: Duration::ZERO,
                ..READ_LIMITS
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        let con = open_source(&path, READ_LIMITS, Instant::now())?;
        let error = con.query_row::<i64, _, _>(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n",
            [], |row| row.get(0),
        ).unwrap_err();
        assert!(
            matches!(error, rusqlite::Error::SqliteFailure(code, _) if code.code == rusqlite::ErrorCode::OperationInterrupted)
        );
        Ok(())
    }
}

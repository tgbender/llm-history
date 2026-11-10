use crate::Store;
use anyhow::Result;
use llm_history_core::fixture_record;
use llm_history_types::Harness;
use llm_history_types::query::Filter;
use serde_json::json;
use std::fs;
use std::path::Path;

fn transcript(root: &Path, harness: Harness, cwd: &str) -> Result<()> {
    let path = root.join(if harness == Harness::Codex {
        "sessions/a.jsonl"
    } else {
        "projects/p/a.jsonl"
    });
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut records = fixture_record(harness, "same-native", "project history");
    if harness == Harness::Codex {
        records[0]["payload"]["cwd"] = json!(cwd);
    } else {
        for record in &mut records {
            record["cwd"] = json!(cwd);
        }
    }
    std::fs::write(
        path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )?;
    Ok(())
}
#[test]
fn sqlite_catalog_precedes_global_state_and_scopes_identical_projects_and_threads() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("catalog.duckdb"), false)?;
    for name in ["personal", "work"] {
        let root = d.path().join(name);
        transcript(&root, Harness::Codex, "/repos/common/sub")?;
        store.add_profile(name, Harness::Codex, &root, None, None)?;
        std::fs::write(root.join(".codex-global-state.json"),json!({"local-projects":{"same-project":{"id":"same-project","name":"wrong fallback","rootPaths":["/repos/common"]}},"credential":"synthetic-secret"}).to_string())?;
        let con = rusqlite::Connection::open(root.join("state_5.sqlite"))?;
        con.execute_batch(
            "create table projects(id text,name text,position integer,\
             metadata text);\
             create table project_roots(project_id text,position integer,\
             path text);\
             create table threads(id text,project_id text);",
        )?;
        con.execute(
            "insert into projects values ('same-project',?,0,'synthetic-secret')",
            [name],
        )?;
        con.execute_batch(
            "insert into project_roots \
             values ('same-project',0,'/repos/common');\
             insert into threads \
             values ('same-native','same-project');",
        )?;
    }
    let before = std::fs::read(d.path().join("work/state_5.sqlite"))?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(receipt.errors.is_empty(), "{receipt:?}");
    assert_eq!(receipt.catalogs.snapshots, 2);
    assert_eq!(std::fs::read(d.path().join("work/state_5.sqlite"))?, before);
    for name in ["personal", "work"] {
        let messages = store.messages_page(
            &Filter {
                profiles: vec![name.into()],
                project: Some("same-project".into()),
                ..Default::default()
            },
            None,
            usize::MAX,
        )?;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].project, name);
        let rows = store.list_projects_including(
            &Filter {
                profiles: vec![name.into()],
                ..Default::default()
            },
            100,
            false,
        )?;
        assert_eq!(rows[0].name, name);
        assert_eq!(rows[0].native_id, "same-project");
        assert_eq!(rows[0].sessions, 1);
    }
    assert_eq!(
        store.database.connection().query_row(
            "select count(*) from catalog_snapshots where contains(data,'synthetic-secret')",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        0
    );
    let unchanged = crate::tests::fixtures::capture(&store, &[], false)?.catalogs;
    assert_eq!(unchanged.snapshots, 0);
    assert_eq!(unchanged.assignments, 0);
    let project = store.list_projects_including(
        &Filter {
            profiles: vec!["work".into()],
            ..Default::default()
        },
        100,
        false,
    )?[0]
        .id
        .clone();
    let con = rusqlite::Connection::open(d.path().join("work/state_5.sqlite"))?;
    con.execute_batch("PRAGMA journal_mode=WAL")?;
    crate::tests::fixtures::capture(&store, &[], false)?;
    let main_before = std::fs::metadata(d.path().join("work/state_5.sqlite"))?.modified()?;
    con.execute_batch("update projects set name='Renamed work'")?;
    assert_eq!(
        std::fs::metadata(d.path().join("work/state_5.sqlite"))?.modified()?,
        main_before
    );
    assert_eq!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .catalogs
            .snapshots,
        1
    );
    let rows = store.list_projects_including(
        &Filter {
            profiles: vec!["work".into()],
            ..Default::default()
        },
        100,
        false,
    )?;
    assert_eq!(rows[0].id, project);
    assert_eq!(rows[0].name, "Renamed work");
    con.execute_batch(
        "insert into projects \
         values ('unrelated','Unrelated',1,'{}'); \
         insert into project_roots \
         values ('unrelated',0,'/repos/unrelated')",
    )?;
    let unrelated = crate::tests::fixtures::capture(&store, &[], false)?.catalogs;
    assert_eq!(unrelated.snapshots, 1);
    assert_eq!(unrelated.assignments, 0);
    assert_eq!(store.verify()?["catalog_snapshots"], 2);
    Ok(())
}
#[test]
fn claude_uses_longest_registered_root_and_catalogs_exclude_credentials() -> Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("claude-work");
    transcript(&root, Harness::Claude, "/repos/abc/nested")?;
    let path = d.path().join("claude-work.json");
    std::fs::write(&path,json!({"projects":{"/repos":{},"/repos/abc":{"apiKey":"synthetic-secret"}},"githubRepoPaths":{"owner/repo":["/repos/abc","/unregistered"]},"oauthToken":"synthetic-secret"}).to_string())?;
    let store = Store::open(&d.path().join("catalog.duckdb"), false)?;
    store.add_profile("work", Harness::Claude, &root, None, None)?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(result.errors.is_empty(), "{result:?}");
    assert_eq!(
        store
            .messages_page(
                &Filter {
                    project: Some("abc".into()),
                    ..Default::default()
                },
                None,
                usize::MAX
            )?
            .len(),
        1
    );
    let rows = store.list_projects_including(&Filter::default(), 100, false)?;
    let project = rows.iter().find(|r| r.name == "abc").unwrap();
    assert_eq!(project.metadata["repository_slugs"], json!(["owner/repo"]));
    let data: String =
        store
            .database
            .connection()
            .query_row("select data from catalog_snapshots", [], |r| r.get(0))?;
    assert!(!data.contains("synthetic-secret"));
    assert!(!data.contains("/unregistered"));
    std::fs::write(path, "{broken")?;
    let result = crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!(result.errors.len(), 1);
    assert_eq!(
        store
            .messages_page(
                &Filter {
                    project: Some("abc".into()),
                    ..Default::default()
                },
                None,
                usize::MAX
            )?
            .len(),
        1
    );
    Ok(())
}

#[test]
fn explicit_membership_sets_are_searchable_and_retired_when_catalog_changes() -> Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    transcript(&root, Harness::Codex, "/repos/common")?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    let native = rusqlite::Connection::open(root.join("state_5.sqlite"))?;
    native.execute_batch(
        "CREATE TABLE projects(id TEXT,name TEXT,position INTEGER,\
         metadata TEXT);\
         CREATE TABLE project_roots(project_id TEXT,position INTEGER,\
         path TEXT);\
         CREATE TABLE threads(id TEXT,project_id TEXT);\
         INSERT INTO projects \
         VALUES ('one','One',0,'{}'),('two','Two',1,'{}');\
         INSERT INTO project_roots \
         VALUES ('one',0,'/repos/common'),('two',0,'/elsewhere');\
         INSERT INTO threads \
         VALUES ('same-native','one'),('same-native','two')",
    )?;
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");
    for project in ["one", "two"] {
        assert_eq!(
            store
                .messages_page(
                    &Filter {
                        project: Some(project.into()),
                        ..Default::default()
                    },
                    None,
                    usize::MAX
                )?
                .len(),
            1
        );
    }
    assert_eq!(
        store.database.connection().query_row(
            "SELECT count(*) FROM canonical_session_projects WHERE active",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        2
    );
    native.execute_batch("DELETE FROM threads WHERE project_id='one'")?;
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");
    assert!(
        store
            .messages_page(
                &Filter {
                    project: Some("one".into()),
                    ..Default::default()
                },
                None,
                usize::MAX
            )?
            .is_empty()
    );
    assert_eq!(
        store
            .messages_page(
                &Filter {
                    project: Some("two".into()),
                    ..Default::default()
                },
                None,
                usize::MAX
            )?
            .len(),
        1
    );
    let counts = store.list_projects_including(&Filter::default(), 100, false)?;
    assert_eq!(
        counts
            .iter()
            .find(|p| p.native_id == "one")
            .unwrap()
            .sessions,
        0
    );
    assert_eq!(
        counts
            .iter()
            .find(|p| p.native_id == "two")
            .unwrap()
            .sessions,
        1
    );
    Ok(())
}

#[test]
fn native_thread_titles_models_and_effort_are_profile_scoped_and_follow_wal_changes() -> Result<()>
{
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    for name in ["work", "personal"] {
        let root = d.path().join(name);
        transcript(&root, Harness::Codex, "/project")?;
        store.add_profile(name, Harness::Codex, &root, None, None)?;
        let native = rusqlite::Connection::open(root.join("state_5.sqlite"))?;
        native.execute_batch("CREATE TABLE threads(id TEXT,title TEXT,model TEXT,reasoning_effort TEXT,updated_at INTEGER)")?;
        native.execute(
            "INSERT INTO threads VALUES ('same-native',?,?,'high',1)",
            [name, name],
        )?;
    }
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");
    for name in ["work", "personal"] {
        let rows = store.list_sessions(
            &Filter {
                profiles: vec![name.into()],
                ..Default::default()
            },
            100,
            false,
        )?;
        assert_eq!(rows[0].title, name);
        assert_eq!(rows[0].model, name);
        assert_eq!(rows[0].effort, "high");
    }
    let native = rusqlite::Connection::open(d.path().join("work/state_5.sqlite"))?;
    native.execute_batch("PRAGMA journal_mode=WAL;UPDATE threads SET title='Updated work',reasoning_effort='low',updated_at=2")?;
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");
    let rows = store.list_sessions(
        &Filter {
            profiles: vec!["work".into()],
            ..Default::default()
        },
        100,
        false,
    )?;
    assert_eq!(rows[0].title, "Updated work");
    assert_eq!(rows[0].effort, "low");
    Ok(())
}

#[test]
fn failed_thread_acquisition_retains_metadata_and_reports_committed_transcripts() -> Result<()> {
    use std::io::Write;
    let d = tempfile::tempdir()?;
    let root = d.path().join("codex");
    transcript(&root, Harness::Codex, "/project")?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    let profile = store.add_profile("work", Harness::Codex, &root, None, None)?;
    let native = rusqlite::Connection::open(root.join("state_5.sqlite"))?;
    native.execute_batch("CREATE TABLE threads(id TEXT,title TEXT); INSERT INTO threads VALUES ('same-native','Retained title')")?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let key = format!("catalog:{}", profile.id);
    let previous: String = store.database.connection().query_row(
        "SELECT value FROM meta WHERE key=?",
        [&key],
        |r| r.get(0),
    )?;
    native.execute_batch("UPDATE threads SET title=x'80'")?;
    let record = fixture_record(Harness::Codex, "same-native", "newly captured text").remove(1);
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(root.join("sessions/a.jsonl"))?,
        "{record}"
    )?;
    let receipt = crate::tests::fixtures::capture(&store, &[], false)?;
    assert_eq!(receipt.errors.len(), 1, "{receipt:?}");
    assert!(receipt.errors[0].contains("previous metadata retained"));
    assert_eq!(
        store
            .messages_page(&Filter::default(), None, usize::MAX)?
            .len(),
        2
    );
    assert_eq!(
        store.list_sessions(&Filter::default(), 10, false)?[0].title,
        "Retained title"
    );
    assert_eq!(
        store.database.connection().query_row(
            "SELECT value FROM meta WHERE key=?",
            [&key],
            |r| r.get::<_, String>(0)
        )?,
        previous
    );
    // A failed capture leaves no destination transaction open and retries after recovery.
    native.execute_batch("UPDATE threads SET title='Recovered title'")?;
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    assert_eq!(
        store.list_sessions(&Filter::default(), 10, false)?[0].title,
        "Recovered title"
    );
    Ok(())
}

#[test]
fn sessions_prefer_current_native_model_effort_and_preview() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    let root = d.path().join("work");
    let path = root.join("sessions/a.jsonl");
    fs::create_dir_all(path.parent().unwrap())?;
    let mut records = fixture_record(Harness::Codex, "same-native", "project history");
    records.insert(
        1,
        json!({"type":"turn_context","payload":{"model":"old-model","effort":"low"}}),
    );
    fs::write(
        &path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )?;
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    let native = rusqlite::Connection::open(root.join("state_5.sqlite"))?;
    native.execute_batch(
        "CREATE TABLE threads(id TEXT,title TEXT,model TEXT,\
         reasoning_effort TEXT,preview TEXT,updated_at INTEGER);\
         INSERT INTO threads \
         VALUES ('same-native','','new-model','high','thread preview',1)",
    )?;
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");
    let rows = store.list_sessions(&Filter::default(), 100, false)?;
    assert_eq!(rows[0].title, "thread preview");
    assert_eq!(rows[0].model, "new-model");
    assert_eq!(rows[0].effort, "high");
    Ok(())
}

#[test]
fn codex_titles_prefer_thread_names_over_the_first_message() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    let root = d.path().join("work");
    let long = format!("{} with more detail\nand a second line", "x".repeat(150));
    // id, name, title, preview, first message, agent path
    let threads = [
        (
            "named",
            "Short generated name",
            "first ask",
            "first ask",
            "first ask",
            "",
        ),
        (
            "unnamed",
            "",
            "Fix the importer\n\nlong pasted log",
            "",
            "Fix the importer\n\nlong pasted log",
            "",
        ),
        ("long", "", &long, &long, &long, ""),
        ("agent", "", "", "", "", "/root/parity_audit"),
    ];
    fs::create_dir_all(root.join("sessions"))?;
    for (id, ..) in &threads {
        let records = fixture_record(Harness::Codex, id, "prompt");
        fs::write(
            root.join(format!("sessions/{id}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    store.add_profile("work", Harness::Codex, &root, None, None)?;
    let native = rusqlite::Connection::open(root.join("state_5.sqlite"))?;
    native.execute_batch(
        "CREATE TABLE threads(id TEXT,name TEXT,title TEXT,preview TEXT,\
         first_user_message TEXT,agent_path TEXT,updated_at INTEGER)",
    )?;
    for (id, name, title, preview, first, path) in &threads {
        native.execute(
            "INSERT INTO threads VALUES (?,?,?,?,?,?,1)",
            [id, name, title, preview, first, path],
        )?;
    }
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");
    let filter = Filter {
        include_subagents: true,
        ..Default::default()
    };
    let rows = store.list_sessions(&filter, 100, false)?;
    let titles = rows
        .iter()
        .map(|r| (r.thread_id.clone(), r.title.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(titles["named"], "Short generated name");
    assert_eq!(titles["unnamed"], "Fix the importer");
    assert_eq!(titles["long"], format!("{}…", "x".repeat(119)));
    assert_eq!(titles["agent"], "parity_audit");
    Ok(())
}

#[test]
fn claude_sessions_answers_and_titles_follow_python() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    let root = d.path().join("claude");
    let session = root.join("projects/p/s1.jsonl");
    let subagent = root.join("projects/p/s1/subagents/agent-abc.jsonl");
    fs::create_dir_all(subagent.parent().unwrap())?;
    let records = [
        json!({"type":"user","timestamp":"2026-10-01T00:00:01Z","sessionId":"s1","uuid":"u1","message":{"role":"user","content":"how do I build the widget"}}),
        json!({"type":"assistant","timestamp":"2026-10-01T00:00:02Z","sessionId":"s1","message":{"id":"m1","role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1","name":"Read","input":{}}]}}),
        json!({"type":"user","timestamp":"2026-10-01T00:00:03Z","sessionId":"s1","uuid":"u2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"file"}]}}),
        json!({"type":"assistant","timestamp":"2026-10-01T00:00:04Z","sessionId":"s1","message":{"id":"m2","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"final widget answer"}]}}),
        json!({"type":"last-prompt","sessionId":"s1","lastPrompt":"latest prompt"}),
    ];
    fs::write(
        &session,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )?;
    fs::write(
        &subagent,
        format!(
            "{}\n",
            json!({"type":"user","timestamp":"2026-10-01T00:00:05Z","sessionId":"s1","message":{"role":"user","content":"delegated task"}})
        ),
    )?;
    store.add_profile("claude", Harness::Claude, &root, None, None)?;
    let updated = crate::tests::fixtures::capture(&store, &[], false)?;
    assert!(updated.errors.is_empty(), "{updated:?}");

    // Tool results are text-free user records, not a new question.
    let question = store
        .messages_page(&Filter::default(), None, usize::MAX)?
        .into_iter()
        .find(|m| m.text.contains("widget") && m.role == "user")
        .unwrap();
    let answers = store.answer_messages(&[question.id.parse()?], &Filter::default())?;
    assert_eq!(answers[&question.id].text, "final widget answer");

    let filter = Filter {
        include_subagents: true,
        ..Default::default()
    };
    let rows = store.list_sessions(&filter, 100, true)?;
    assert_eq!(rows[0].title, "latest prompt");
    assert_eq!(rows[0].role, "");
    assert_eq!(
        rows[0].created_at.as_deref(),
        Some("2026-10-01T00:00:01.000Z")
    );
    assert_eq!(rows[1].title, "delegated task");
    assert_eq!(rows[1].role, "subagent");
    // Listed timestamps are accepted back as bounds.
    let since = Filter {
        since: rows[1].created_at.clone(),
        ..filter
    };
    assert_eq!(store.list_sessions(&since, 100, true)?.len(), 1);
    Ok(())
}

#[test]
fn project_filters_reject_ambiguous_names_within_the_selected_profiles() -> Result<()> {
    let d = tempfile::tempdir()?;
    let store = Store::open(&d.path().join("history.duckdb"), false)?;
    for (name, roots) in [
        ("work", vec!["/one/demo", "/two/demo"]),
        ("personal", vec!["/three/demo"]),
    ] {
        let root = d.path().join(name);
        fs::create_dir_all(root.join("sessions"))?;
        store.add_profile(name, Harness::Codex, &root, None, None)?;
        for (index, cwd) in roots.into_iter().enumerate() {
            let records = [
                json!({"type":"session_meta","payload":{"id":format!("{name}-{index}"),"cwd":cwd}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":"needle"}}),
            ];
            fs::write(
                root.join(format!("sessions/{index}.jsonl")),
                records.iter().map(|v| format!("{v}\n")).collect::<String>(),
            )?;
        }
    }
    assert!(
        crate::tests::fixtures::capture(&store, &[], false)?
            .errors
            .is_empty()
    );
    let mut filter = Filter {
        profiles: vec!["work".into()],
        project: Some("demo".into()),
        ..Default::default()
    };
    assert!(
        store
            .validate_project_filter(&filter)
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    filter.profiles = vec!["personal".into()];
    store.validate_project_filter(&filter)?;
    filter.project = Some("missing".into());
    assert!(
        store
            .validate_project_filter(&filter)
            .unwrap_err()
            .to_string()
            .contains("Unknown project")
    );
    filter.profiles = vec!["work".into()];
    let id: String = store.database.connection().query_row(
        "SELECT native_project_id \
         FROM canonical_projects \
         WHERE store_id=(\
         SELECT id \
         FROM profiles \
         WHERE name='work') AND active \
         LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    filter.project = Some(id);
    store.validate_project_filter(&filter)?;
    Ok(())
}

//! Agents read the text tables through pipes, so cells must arrive whole.
use anyhow::Result;
use llm_history_core::fixture_record;
use llm_history_types::Harness;
use serde_json::json;
use std::fs;
use std::process::{Command, Output};

fn run(home: &std::path::Path, args: &[&str]) -> Result<Output> {
    let db = home.join("history.duckdb");
    Ok(Command::new(env!("CARGO_BIN_EXE_llm-history"))
        .arg("--db")
        .arg(&db)
        .args(args)
        .env("HOME", home)
        .env("CODEX_HOME", home.join("codex"))
        .env("CLAUDE_CONFIG_DIR", home.join("no-claude"))
        .env("COLUMNS", "60")
        .output()?)
}

#[test]
fn search_context_and_similarity_share_compact_message_ids() -> Result<()> {
    use llm_history_store::{EmbeddingBatch, EmbeddingDocument, Store};
    use llm_history_types::query::Filter;

    let dir = tempfile::tempdir()?;
    let cli = |args: &[&str]| -> Result<serde_json::Value> {
        let out = run(dir.path(), &[&["--json"], args].concat())?;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(serde_json::from_slice(&out.stdout)?)
    };
    for (profile, messages) in [
        (
            "work",
            &["unique anchor", "closest", "next", "unembedded"][..],
        ),
        ("personal", &["other profile"][..]),
    ] {
        let root = dir.path().join(profile);
        fs::create_dir_all(root.join("sessions"))?;
        for (index, text) in messages.iter().enumerate() {
            let mut records =
                fixture_record(Harness::Codex, &format!("native-thread-{index}"), text);
            if *text == "unique anchor" {
                let same_thread = fixture_record(Harness::Codex, "native-thread-0", "same thread");
                records.extend(same_thread.into_iter().skip(1));
            }
            if *text == "next" {
                records.push(json!({"type":"response_item","payload":{"type":"message",
                    "role":"assistant","phase":"final_answer",
                    "content":[{"type":"output_text","text":"assistant echo"}]}}));
            }
            fs::write(
                root.join(format!("sessions/{index}.jsonl")),
                records.iter().map(|r| format!("{r}\n")).collect::<String>(),
            )?;
        }
        cli(&[
            "profile",
            "add",
            profile,
            "--harness",
            "codex",
            "--root",
            root.to_str().unwrap(),
        ])?;
    }
    cli(&["update", "--no-background"])?;
    let hit = |text: &str| -> Result<serde_json::Value> {
        Ok(cli(&[
            "search",
            "--mode",
            "exact",
            "--no-refresh",
            "--no-background",
            text,
        ])?["results"][0]
            .clone())
    };
    let anchor = hit("unique anchor")?;
    let id = anchor["id"].as_i64().unwrap().to_string();
    assert_eq!(anchor.as_object().unwrap().len(), 7);
    assert_eq!(cli(&["show", &id])?["messages"][0]["id"], anchor["id"]);
    let no_vectors = run(dir.path(), &["similar", &id])?;
    assert!(!no_vectors.status.success());
    assert!(String::from_utf8_lossy(&no_vectors.stderr).contains("semantic backfill"));

    {
        let store = Store::open(&dir.path().join("history.duckdb"), false)?;
        let vector = |x, y| {
            let mut v = vec![0.; 768];
            v[0] = x;
            v[1] = y;
            v
        };
        let mut documents = Vec::new();
        for m in store.messages_page(&Filter::default(), None, 20)? {
            let vectors = match m.text.as_str() {
                "unique anchor" => vec![vector(1., 0.), vector(0., 1.)],
                "closest" => vec![vector(0., 1.), vector(0.6, 0.8)],
                "next" => vec![vector(0.6, 0.8)],
                "same thread" | "assistant echo" => vec![vector(1., 0.)],
                "other profile" => vec![vector(1., 0.)],
                _ => continue,
            };
            documents.push(EmbeddingDocument {
                id: m.id.parse()?,
                text_hash: m.text_hash,
                vectors,
                failure: None,
            });
        }
        let model = llm_history_models::gemma_profile(768)?;
        store.publish_embedding_batch(
            &model.id,
            &serde_json::to_string(&model)?,
            &EmbeddingBatch::new(&documents)?,
        )?;
    }
    let user_hits = cli(&["similar", &id, "--role", "user"])?;
    let user_hits = user_hits["results"].as_array().unwrap();
    assert!(user_hits.iter().any(|r| r["text"] == "same thread"));
    assert!(user_hits.iter().all(|r| r["role"] == "user"));
    let assistant_hits = cli(&["similar", &id, "--role", "assistant", "--other-sessions"])?;
    assert_eq!(assistant_hits["results"].as_array().unwrap().len(), 1);
    assert_eq!(assistant_hits["results"][0]["text"], "assistant echo");
    let filters = ["similar", &id, "--role", "user", "--other-sessions"];
    let found = cli(&filters)?;
    let rows = found["results"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["text"], "closest");
    assert_eq!(rows[0]["similarity"], 1.0);
    assert_eq!(rows[1]["text"], "next");
    assert_eq!(rows[1]["similarity"], 0.8);
    assert_eq!(rows[0]["id"], hit("closest")?["id"]);
    assert!(
        rows.iter()
            .all(|r| r["profile"] == "work" && r.get("thread_id").is_none())
    );
    let across = cli(&["similar", &id, "--profile", "personal"])?;
    assert_eq!(across["results"].as_array().unwrap().len(), 1);
    assert_eq!(across["results"][0]["text"], "other profile");
    let different_space = run(dir.path(), &["similar", &id, "--dimensions", "256"])?;
    assert!(!different_space.status.success());
    cli(&["db", "compact"])?;
    assert_eq!(cli(&filters)?, found);
    Ok(())
}

#[test]
fn sessions_cells_are_not_shortened_in_a_pipe() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let cwd = format!("/projects/{}", "deep/".repeat(30));
    let title =
        "An ask that runs past both the eighty and sixty column marks on one line and keeps";
    let secret = "sk-abcdefghijklmnopqrstuv";
    assert!(title.len() > 80 && title.len() + secret.len() < 120);
    let mut records = fixture_record(Harness::Codex, "thread-1", "prompt");
    records[0]["payload"]["cwd"] = json!(cwd);
    let path = dir.path().join("codex/sessions/a.jsonl");
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(
        &path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )?;
    let native = rusqlite::Connection::open(dir.path().join("codex/state_5.sqlite"))?;
    native
        .execute_batch("CREATE TABLE threads(id TEXT,name TEXT,model TEXT,updated_at INTEGER)")?;
    native.execute(
        "INSERT INTO threads VALUES ('thread-1',?,'m',1)",
        [format!("{title} {secret}")],
    )?;
    drop(native);

    let updated = run(dir.path(), &["update", "--no-background"])?;
    assert!(updated.status.success(), "{updated:?}");
    let listed = run(dir.path(), &["sessions"])?;
    assert!(listed.status.success(), "{listed:?}");
    let stdout = String::from_utf8(listed.stdout)?;
    assert!(stdout.contains(&format!("{title} [REDACTED]")), "{stdout}");
    assert!(!stdout.contains(secret), "{stdout}");
    assert!(!stdout.contains(&cwd), "{stdout}");
    assert!(!stdout.contains("thread-1"), "{stdout}");
    assert!(!stdout.contains('…'), "{stdout}");
    Ok(())
}

#[test]
fn a_bad_timezone_fails_before_any_work_and_receipts_keep_their_nesting() -> Result<()> {
    let dir = tempfile::tempdir()?;
    for args in [
        &[
            "search",
            "q",
            "--no-background",
            "--timezone",
            "Mars/Olympus",
        ][..],
        &["sessions", "--timezone", "Mars/Olympus"],
        &["usage", "--timezone", "Mars/Olympus"],
    ] {
        let rejected = run(dir.path(), args)?;
        assert!(!rejected.status.success(), "{rejected:?}");
        let stderr = String::from_utf8(rejected.stderr)?;
        assert!(
            stderr.contains("invalid IANA timezone: Mars/Olympus"),
            "{stderr}"
        );
    }
    assert!(!dir.path().join("history.duckdb").exists());

    let updated = run(dir.path(), &["update", "--no-background"])?;
    assert!(updated.status.success(), "{updated:?}");
    let stdout = String::from_utf8(updated.stdout)?;
    assert!(
        stdout.contains("catalogs:\n  assignments: 0\n  errors: none\n  projects: 0\n"),
        "{stdout}"
    );
    assert!(stdout.contains("\nerrors: none\nimported: 0\n"), "{stdout}");
    Ok(())
}

#[test]
fn show_json_identifies_the_provider_once_for_the_context() -> Result<()> {
    for harness in [Harness::Codex, Harness::Claude] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(if harness == Harness::Codex {
            "codex/sessions/a.jsonl"
        } else {
            "claude/projects/p/a.jsonl"
        });
        fs::create_dir_all(path.parent().unwrap())?;
        let records = ["before", "needle", "after"]
            .into_iter()
            .flat_map(|text| fixture_record(harness, "s", text))
            .map(|r| format!("{r}\n"))
            .collect::<String>();
        fs::write(&path, records)?;
        if harness == Harness::Claude {
            let out = run(
                dir.path(),
                &[
                    "profile",
                    "add",
                    "work",
                    "--harness",
                    "claude",
                    "--root",
                    dir.path().join("claude").to_str().unwrap(),
                ],
            )?;
            assert!(out.status.success(), "{out:?}");
        }
        let out = run(dir.path(), &["update", "--no-background"])?;
        assert!(out.status.success(), "{out:?}");
        let found = run(
            dir.path(),
            &[
                "--json",
                "search",
                "needle",
                "--no-refresh",
                "--no-background",
            ],
        )?;
        assert!(found.status.success(), "{found:?}");
        let found: serde_json::Value = serde_json::from_slice(&found.stdout)?;
        let id = found["results"][0]["id"].as_i64().unwrap().to_string();
        let shown = run(
            dir.path(),
            &["--json", "show", &id, "--before", "1", "--after", "1"],
        )?;
        assert!(shown.status.success(), "{shown:?}");
        let shown: serde_json::Value = serde_json::from_slice(&shown.stdout)?;
        let rows = shown["messages"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(shown["source"], harness.key(), "{shown}");
        assert!(
            rows.iter()
                .all(|row| row.get("thread_id").is_none() && row.get("path").is_none())
        );
        assert_eq!(rows[1]["target"], true);
    }
    Ok(())
}

#[test]
fn exact_search_runs_through_the_cli() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("codex/sessions/a.jsonl");
    fs::create_dir_all(path.parent().unwrap())?;
    for (at, text) in ["git push --force", "git force push"].iter().enumerate() {
        let records = fixture_record(Harness::Codex, &format!("thread-{at}"), text);
        fs::write(
            path.with_file_name(format!("{at}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    let updated = run(dir.path(), &["update", "--no-background"])?;
    assert!(updated.status.success(), "{updated:?}");
    let found = run(
        dir.path(),
        &["--json", "search", "--mode", "exact", "GIT PUSH --FORCE"],
    )?;
    assert!(found.status.success(), "{found:?}");
    let found: serde_json::Value = serde_json::from_slice(&found.stdout)?;
    assert_eq!(found["results"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["results"][0]["text"], "git push --force");
    // A flag is a natural thing to look for, so it must reach the search as the query.
    let flag = run(
        dir.path(),
        &["--json", "search", "--mode", "exact", "--force"],
    )?;
    assert!(flag.status.success(), "{flag:?}");
    let flag: serde_json::Value = serde_json::from_slice(&flag.stdout)?;
    assert_eq!(flag["results"].as_array().unwrap().len(), 1, "{flag}");
    // This home has no inference files, so a command that started a model would fail.
    let semantic = run(
        dir.path(),
        &[
            "--json",
            "semantic",
            "search",
            "--mode",
            "exact",
            "GIT PUSH --FORCE",
        ],
    )?;
    assert!(semantic.status.success(), "{semantic:?}");
    let semantic: serde_json::Value = serde_json::from_slice(&semantic.stdout)?;
    assert_eq!(
        semantic["results"].as_array().unwrap().len(),
        1,
        "{semantic}"
    );
    assert_eq!(semantic["results"][0]["text"], "git push --force");
    let refused = run(dir.path(), &["search", "--mode", "exact", "x", "--rerank"])?;
    assert!(!refused.status.success(), "{refused:?}");
    let refused = String::from_utf8(refused.stderr)?;
    assert!(refused.contains("--rerank"), "{refused}");
    assert!(!refused.contains("inference file"), "{refused}");
    let undone = ["search", "--mode", "exact", "x", "--rerank", "--no-rerank"];
    let undone = run(dir.path(), &undone)?;
    assert!(undone.status.success(), "{undone:?}");
    Ok(())
}

#[test]
fn a_plain_search_finds_every_word_in_what_the_user_typed() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let sessions = dir.path().join("codex/sessions");
    fs::create_dir_all(&sessions)?;
    for at in 0..9 {
        let mut records = fixture_record(
            Harness::Codex,
            &format!("thread-{at}"),
            "warm the lunar cache",
        );
        if at == 0 {
            records.push(
                json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z",
                "payload":{"type":"message","role":"assistant","phase":"final_answer",
                "content":[{"type":"output_text","text":"the cache is lunar now"}]}}),
            );
        }
        fs::write(
            sessions.join(format!("{at}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    let updated = run(dir.path(), &["update", "--no-background"])?;
    assert!(updated.status.success(), "{updated:?}");
    let json = |flags: &[&str]| -> Result<serde_json::Value> {
        let out = run(dir.path(), &[&["--json", "search"], flags].concat())?;
        assert!(out.status.success(), "{out:?}");
        Ok(serde_json::from_slice(&out.stdout)?)
    };
    let roles = |found: &serde_json::Value| -> Vec<String> {
        found["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["role"].as_str().unwrap().to_owned())
            .collect()
    };
    // Eight of the nine questions and no answer, with the words in another order.
    let plain = json(&["CACHE lun"])?;
    assert_eq!(roles(&plain), ["user"; 8], "{plain}");
    assert_eq!(plain["matched"], 9);
    let all = json(&["CACHE lun", "--role", "all", "--limit", "20"])?;
    assert_eq!(all["results"].as_array().unwrap().len(), 10, "{all}");
    let answers = json(&["CACHE lun", "--role", "assistant"])?;
    assert_eq!(roles(&answers), ["assistant"], "{answers}");
    assert_eq!(answers["results"][0]["text"], "the cache is lunar now");
    // The same words as one literal string appear nowhere, and the result says what it covered.
    let exact = json(&["--mode", "exact", "CACHE lun"])?;
    assert!(exact["results"].as_array().unwrap().is_empty(), "{exact}");
    assert_eq!(exact["searched"]["messages"], 9);
    assert_eq!(exact["searched"]["through"], "2026-10-01T00:00:02Z");
    assert!(plain.get("searched").is_none());
    let nothing = stdout(dir.path(), &["search", "--mode", "exact", "CACHE lun"])?;
    assert_eq!(
        nothing,
        "No matches in 9 user messages; archive through 2026-10-01T00:00:02Z.\n"
    );
    // Dates read as in git log, and habits from search don't fail a command that only reads.
    let dated = json(&["cache", "--after", "2026-09-30", "--before", "2026-10-02"])?;
    assert_eq!(dated["matched"], 9);
    let later = json(&["cache", "--after", "2026-10-02"])?;
    assert!(later["results"].as_array().unwrap().is_empty(), "{later}");
    for habit in [
        &["status", "--no-refresh"][..],
        &["sessions", "--no-refresh", "--no-background"],
        &["sql", "select 1", "--no-refresh"],
    ] {
        assert!(run(dir.path(), habit)?.status.success(), "{habit:?}");
    }
    Ok(())
}

/// A Codex session: name, working directory, and messages as (role, text).
type Session<'a> = (&'a str, &'a str, Vec<(&'a str, String)>);

/// Writes and imports the sessions, with messages one second apart.
fn write_sessions(home: &std::path::Path, sessions: &[Session]) -> Result<()> {
    let root = home.join("codex/sessions");
    fs::create_dir_all(&root)?;
    for (name, cwd, messages) in sessions {
        let mut records = vec![
            json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z",
            "payload":{"id":name,"cwd":cwd}}),
        ];
        for (at, (role, text)) in messages.iter().enumerate() {
            let (kind, phase) = if *role == "user" {
                ("input_text", None)
            } else {
                ("output_text", Some("final_answer"))
            };
            records.push(json!({"type":"response_item",
                "timestamp":format!("2026-10-01T00:{:02}:{:02}Z", at / 60, at % 60),
                "payload":{"type":"message","role":role,"phase":phase,
                "content":[{"type":kind,"text":text}]}}));
        }
        fs::write(
            root.join(format!("{name}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    let updated = run(home, &["update", "--no-background"])?;
    assert!(updated.status.success(), "{updated:?}");
    Ok(())
}

fn stdout(home: &std::path::Path, args: &[&str]) -> Result<String> {
    let out = run(home, args)?;
    assert!(out.status.success(), "{out:?}");
    Ok(String::from_utf8(out.stdout)?)
}

#[test]
fn sessions_filter_by_working_directory_and_reject_unknown_projects() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let said = |text: &str| vec![("user", text.to_owned())];
    write_sessions(
        dir.path(),
        &[
            ("a", "/projects/alpha", said("first")),
            ("b", "/projects/alpha/tools", said("second")),
            ("c", "/projects/beta", said("third")),
        ],
    )?;
    let count = |flags: &[&str]| -> Result<usize> {
        let listed = stdout(dir.path(), &[&["--json", "sessions"], flags].concat())?;
        let listed: serde_json::Value = serde_json::from_str(&listed)?;
        Ok(listed.as_array().unwrap().len())
    };
    assert_eq!(count(&[])?, 3);
    assert_eq!(count(&["--under", "/projects/alpha"])?, 2);
    assert_eq!(count(&["--cwd", "/projects/alpha"])?, 1);
    assert_eq!(count(&["--under", "/projects/gamma"])?, 0);
    let unknown = run(dir.path(), &["sessions", "--project", "nope"])?;
    assert!(!unknown.status.success(), "{unknown:?}");
    assert!(String::from_utf8(unknown.stderr)?.contains("Unknown project"));
    Ok(())
}

#[test]
fn show_takes_several_ids_and_shares_one_budget() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let long = "padding ".repeat(400);
    let turns = |name: &str| -> Vec<(&str, String)> {
        (1..=6)
            .map(|at| ("user", format!("{name} turn {at} {long}")))
            .collect()
    };
    write_sessions(
        dir.path(),
        &[
            ("a", "/projects/a", turns("alpha")),
            ("b", "/projects/b", turns("beta")),
        ],
    )?;
    let id = |text: &str| -> Result<String> {
        let found = stdout(dir.path(), &["--json", "search", "--mode", "exact", text])?;
        let found: serde_json::Value = serde_json::from_str(&found)?;
        Ok(found["results"][0]["id"].as_i64().unwrap().to_string())
    };
    let (third, fourth, other) = (id("alpha turn 3")?, id("alpha turn 4")?, id("beta turn 1")?);
    // Two messages either side by default; neighbors in one session print once.
    let shown = stdout(dir.path(), &["show", &third, &fourth, &other])?;
    for turn in 1..=6 {
        assert_eq!(
            shown.matches(&format!("alpha turn {turn}")).count(),
            1,
            "{shown}"
        );
    }
    assert_eq!(
        shown.matches("\n*  ").count() + usize::from(shown.starts_with("*  ")),
        3
    );
    assert!(
        shown.contains("beta turn 3") && !shown.contains("beta turn 4"),
        "{shown}"
    );
    // Nine long messages share the default budget, where each alone exceeds a ninth of it.
    assert!(shown.chars().count() < 13_500, "{}", shown.chars().count());
    let json = stdout(dir.path(), &["--json", "show", &third, &other, "-C", "0"])?;
    let json: serde_json::Value = serde_json::from_str(&json)?;
    let contexts = json["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 2, "{json}");
    assert_eq!(contexts[1]["messages"][0]["target"], true);
    let missing = run(dir.path(), &["show", &third, "999999"])?;
    assert!(!missing.status.success());
    assert!(String::from_utf8(missing.stderr)?.contains("message 999999"));
    Ok(())
}

#[test]
fn sql_ids_are_the_ids_the_commands_take() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let turns = |name: &str| -> Vec<(&str, String)> {
        vec![
            ("user", format!("{name} question")),
            ("assistant", format!("{name} answer")),
        ]
    };
    write_sessions(
        dir.path(),
        &[
            ("a", "/projects/a", turns("alpha")),
            ("b", "/projects/b", turns("beta")),
        ],
    )?;
    let rows = |query: &str| -> Result<Vec<serde_json::Value>> {
        let out = stdout(dir.path(), &["--json", "sql", query])?;
        let out: serde_json::Value = serde_json::from_str(&out)?;
        Ok(out["rows"].as_array().unwrap().clone())
    };
    let found = stdout(dir.path(), &["--json", "search", "beta question"])?;
    let found: serde_json::Value = serde_json::from_str(&found)?;
    let hit = &found["results"][0];
    let message = &rows(
        "select message_id, session_id, event_id, session_key from history_messages \
         where text = 'beta question'",
    )?[0];
    // The natural column names give the short IDs; the long keys have their own names.
    assert_eq!(message["message_id"], hit["id"]);
    assert_eq!(message["session_id"], hit["session_id"]);
    assert!(message["event_id"].as_i64().unwrap() > 1_000_000);
    assert!(message["session_key"].as_i64().unwrap() > 1_000_000);
    let id = message["message_id"].as_i64().unwrap().to_string();
    let session = message["session_id"].as_i64().unwrap().to_string();
    assert!(stdout(dir.path(), &["show", &id])?.contains("beta question"));
    assert!(stdout(dir.path(), &["show", "--session", &session])?.contains("beta answer"));
    // Views join on the short ID, and the macros take it.
    let count = |query: &str| -> Result<i64> { Ok(rows(query)?[0]["n"].as_i64().unwrap()) };
    assert_eq!(
        count("select count(*) n from history_messages join history_sessions using (session_id)")?,
        4
    );
    assert_eq!(
        count(&format!(
            "select count(*) n from history_messages_for_session({session})"
        ))?,
        2
    );
    assert_eq!(
        count(&format!(
            "select count(*) n from history_message_context({id}, 1, 1)"
        ))?,
        2
    );
    assert_eq!(
        count(&format!(
            "select count(*) n from history_descendant_sessions({session})"
        ))?,
        1
    );
    let pair = &rows(&format!(
        "select question_message_id, answer_message_id from history_turn_pairs \
         where session_id = {session}"
    ))?[0];
    assert_eq!(pair["question_message_id"], hit["id"]);
    assert!(pair["answer_message_id"].as_i64().unwrap() < 1_000_000);
    Ok(())
}

#[test]
fn a_session_reads_as_its_turns_within_a_budget() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut messages: Vec<(&str, String)> = (1..=12)
        .map(|at| ("user", format!("turn {at} {}", "words ".repeat(60))))
        .collect();
    messages[1].1 = format!("pasted {} then the question", "log ".repeat(1000));
    messages.push(("assistant", "where the work stopped".to_owned()));
    write_sessions(dir.path(), &[("long", "/projects/demo", messages)])?;
    let sessions = stdout(dir.path(), &["--json", "sessions"])?;
    let sessions: serde_json::Value = serde_json::from_str(&sessions)?;
    let session = sessions[0]["id"].as_i64().unwrap().to_string();
    let read = |flags: &[&str]| {
        stdout(
            dir.path(),
            &[&["show", "--session", &session], flags].concat(),
        )
    };

    // Everything fits: twelve turns in order, the long one cut in the middle, then the answer.
    let whole = read(&[])?;
    assert!(whole.starts_with("session "), "{whole}");
    assert!(whole.contains("12 user turns"), "{whole}");
    assert!(whole.find("turn 1 ").unwrap() < whole.find("turn 12 ").unwrap());
    assert!(
        whole.contains("pasted log") && whole.contains("then the question"),
        "{whole}"
    );
    assert!(whole.contains("characters cut"), "{whole}");
    assert!(whole.contains("\nanswer  ") && whole.contains("where the work stopped"));
    assert!(!whole.contains("skipped"), "{whole}");
    assert!(whole.contains("show ID --full"), "{whole}");

    // A smaller budget keeps the opening turns and the latest ones.
    let tight = read(&["--max-output-chars", "2500"])?;
    assert!(
        tight.contains("turn 1 ") && tight.contains("turn 12 "),
        "{tight}"
    );
    assert!(
        tight.contains("] skipped") || tight.contains(" skipped, ids "),
        "{tight}"
    );
    assert!(!tight.contains("turn 6 "), "{tight}");
    assert!(tight.contains("where the work stopped"), "{tight}");
    assert!(tight.chars().count() < 3200, "{}", tight.chars().count());

    // The most recent turns on request, after the opening one.
    let recent = read(&["--recent", "2"])?;
    assert!(
        recent.contains("turn 1 ") && recent.contains("[turns 2 to 10 skipped"),
        "{recent}"
    );
    assert!(
        recent.contains("turn 11 ") && recent.contains("turn 12 "),
        "{recent}"
    );
    assert!(recent.contains("3 of 12 turns shown"), "{recent}");

    // A skipped turn's ID continues the reading from there.
    let json = stdout(
        dir.path(),
        &["--json", "show", "--session", &session, "--recent", "2"],
    )?;
    let json: serde_json::Value = serde_json::from_str(&json)?;
    assert_eq!(json["turns"], 12);
    assert_eq!(json["skipped"]["turns"], 9);
    let from = json["skipped"]["last_id"].as_i64().unwrap().to_string();
    let rest = read(&["--from", &from])?;
    assert!(
        rest.contains("\n10  ") && !rest.contains("turn 9 "),
        "{rest}"
    );
    assert!(!rest.contains("skipped"), "{rest}");

    let full = run(dir.path(), &["show", "--session", &session, "--full"])?;
    assert!(!full.status.success(), "{full:?}");
    let unknown = run(dir.path(), &["show", "--session", "999"])?;
    assert!(String::from_utf8(unknown.stderr)?.contains("session not found"));
    Ok(())
}

#[test]
fn exact_search_says_how_many_matches_it_left_out() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("codex/sessions/a.jsonl");
    fs::create_dir_all(path.parent().unwrap())?;
    for at in 0..4 {
        let records = fixture_record(Harness::Codex, &format!("thread-{at}"), "needle");
        fs::write(
            path.with_file_name(format!("{at}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    let updated = run(dir.path(), &["update", "--no-background"])?;
    assert!(updated.status.success(), "{updated:?}");
    let exact = ["search", "--mode", "exact", "needle"];
    let text = |flags: &[&str]| -> Result<String> {
        let out = run(dir.path(), &[&exact, flags].concat())?;
        assert!(out.status.success(), "{out:?}");
        Ok(String::from_utf8(out.stdout)?)
    };
    let capped = text(&["--limit", "3"])?;
    assert!(
        capped.contains(
            "3 of 4 matches shown; raise --limit, --per-session or --max-output-chars for more"
        ),
        "{capped}"
    );
    let complete = text(&[])?;
    assert!(!complete.contains("matches shown"), "{complete}");
    let json = |flags: &[&str]| -> Result<serde_json::Value> {
        let out = run(dir.path(), &[&["--json"], flags].concat())?;
        assert!(out.status.success(), "{out:?}");
        Ok(serde_json::from_slice(&out.stdout)?)
    };
    let capped = json(&[&exact[..], &["--limit", "3"]].concat())?;
    assert_eq!(capped["matched"], 4);
    assert_eq!(capped["results"].as_array().unwrap().len(), 3);
    assert!(capped.get("shown").is_none());
    let complete = json(&exact)?;
    assert_eq!(complete["results"].as_array().unwrap().len(), 4);
    assert!(complete.get("matched").is_none() && complete.get("shown").is_none());
    // Ranked modes have no such total, so they don't claim one.
    let ranked = json(&["search", "--mode", "lexical", "needle"])?;
    assert!(ranked.get("matched").is_none() && ranked.get("shown").is_none());
    Ok(())
}

#[test]
fn show_keeps_the_action_of_a_long_guardian_prompt() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let block = ">>> APPROVAL REQUEST START\nPlanned action JSON:\n{\"command\":\"needle\"}\n\
                 >>> APPROVAL REQUEST END\n";
    let sessions = dir.path().join("codex/sessions");
    fs::create_dir_all(&sessions)?;
    for (name, history) in [("long", "padding ".repeat(2000)), ("short", "brief".into())] {
        let prompt = format!("{name} transcript {history}\n{block}");
        let records = [
            json!({"type":"session_meta","payload":{"id":name,"source":{"subagent":{"other":"guardian"}}}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user",
                "content":[{"type":"input_text","text":prompt}]}}),
        ];
        fs::write(
            sessions.join(format!("{name}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    let updated = run(dir.path(), &["update", "--no-background"])?;
    assert!(updated.status.success(), "{updated:?}");
    let found = run(
        dir.path(),
        &["--json", "search", "--kind", "guardian", "needle"],
    )?;
    assert!(found.status.success(), "{found:?}");
    let found: serde_json::Value = serde_json::from_slice(&found.stdout)?;
    let shown = |id: &str, args: &[&str]| -> Result<String> {
        let out = run(dir.path(), &[&["show", id], args].concat())?;
        assert!(out.status.success(), "{out:?}");
        Ok(String::from_utf8(out.stdout)?)
    };
    for hit in found["results"].as_array().unwrap() {
        let id = hit["id"].as_i64().unwrap().to_string();
        let (bounded, full) = (shown(&id, &[])?, shown(&id, &["--full"])?);
        assert!(bounded.contains("needle"), "{bounded}");
        assert!(
            full.contains("needle") && full.contains("transcript"),
            "{full}"
        );
        if full.contains("long transcript") {
            assert!(!bounded.contains("padding"), "{bounded}");
            assert!(bounded.contains("--full"), "{bounded}");
        } else {
            assert!(bounded.contains("short transcript"), "{bounded}");
        }
    }
    assert_eq!(found["results"].as_array().unwrap().len(), 2);
    Ok(())
}

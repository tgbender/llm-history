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
    let ranked = json(&["search", "needle"])?;
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
    for (name, history) in [("long", "padding ".repeat(1000)), ("short", "brief".into())] {
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

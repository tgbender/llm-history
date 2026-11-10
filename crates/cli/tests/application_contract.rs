//! Run the public native-input scenario entirely through the CLI.
//! Set LLM_HISTORY_BASELINE to compare a preserved executable with this candidate.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, Output},
};
#[path = "../../../fixtures/contracts/support.rs"]
mod support;
use support::Fixture;

fn run(executable: &Path, fixture: &Fixture, args: &[&str]) -> Result<Output> {
    Ok(command(executable, fixture, args).output()?)
}

fn command(executable: &Path, fixture: &Fixture, args: &[&str]) -> Command {
    let mut cmd = Command::new(executable);
    cmd.args([
        "--db",
        fixture.root().join("history.duckdb").to_str().unwrap(),
        "--cache",
        fixture.root().join("cache").to_str().unwrap(),
    ])
    .args(args)
    .env_clear()
    .env("PATH", "/usr/bin:/bin")
    .env("HOME", fixture.root())
    .env("USERPROFILE", fixture.root())
    .env("XDG_DATA_HOME", fixture.root().join("data"))
    .env("XDG_CACHE_HOME", fixture.root().join("cache"))
    .env("HF_HOME", fixture.root().join("hf"))
    .env("CODEX_HOME", fixture.root().join("no-codex"))
    .env("CLAUDE_CONFIG_DIR", fixture.root().join("no-claude"))
    .env("LLM_HISTORY_RS_BACKGROUND", "off")
    .env("TZ", "UTC")
    .env("LC_ALL", "C")
    .env("COLUMNS", "60");
    cmd
}
fn normalize(value: &mut Value, root: &str, ids: &BTreeMap<String, String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key == "identity" {
                    *value = json!("<database-identity>");
                } else if key == "elapsed_seconds" {
                    *value = json!("<elapsed>");
                } else {
                    normalize(value, root, ids);
                }
            }
        }
        Value::Array(items) => {
            for value in items {
                normalize(value, root, ids);
            }
        }
        Value::String(text) => {
            if let Some(symbol) = ids.get(text) {
                *text = symbol.clone();
            } else {
                *text = text.replace(root, "<fixture>");
            }
        }
        _ => {}
    }
}
fn observe(
    executable: &Path,
    fixture: &Fixture,
    args: &[&str],
    ids: &BTreeMap<String, String>,
    observations: &mut Vec<Value>,
) -> Result<Value> {
    let output = run(executable, fixture, args)?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    let mut value = json!({"exit":output.status.code(),"stdout":serde_json::from_str::<Value>(&stdout).unwrap_or(json!(stdout)),"stderr":stderr});
    let original = value.clone();
    normalize(&mut value, fixture.root().to_str().unwrap(), ids);
    observations.push(json!({"command":args,"result":value}));
    Ok(original)
}
fn successful(
    executable: &Path,
    fixture: &Fixture,
    args: &[&str],
    ids: &BTreeMap<String, String>,
    observations: &mut Vec<Value>,
) -> Result<Value> {
    let value = observe(executable, fixture, args, ids, observations)?;
    ensure!(value["exit"] == 0, "CLI failed: {args:?}: {value}");
    Ok(value["stdout"].clone())
}

fn scenario(executable: &Path) -> Result<Vec<Value>> {
    let fixture = Fixture::new()?;
    let mut observations = Vec::new();
    let mut ids = BTreeMap::new();
    for p in fixture.manifest["profiles"].as_array().unwrap() {
        let name = p["name"].as_str().unwrap();
        let output = run(
            executable,
            &fixture,
            &[
                "--json",
                "profile",
                "add",
                name,
                "--harness",
                p["harness"].as_str().unwrap(),
                "--root",
                fixture
                    .root()
                    .join(p["root"].as_str().unwrap())
                    .to_str()
                    .unwrap(),
            ],
        )?;
        ensure!(output.status.success(), "profile failed: {output:?}");
        let profile: Value = serde_json::from_slice(&output.stdout)?;
        ids.insert(
            profile["id"].as_str().unwrap().into(),
            format!("<profile:{name}>"),
        );
    }
    successful(
        executable,
        &fixture,
        &["--json", "profile", "list"],
        &ids,
        &mut observations,
    )?;
    let mut retained = None;
    for step in fixture.manifest["steps"].as_array().unwrap() {
        match step["operation"].as_str().unwrap() {
            "update" => {
                successful(
                    executable,
                    &fixture,
                    &["--json", "update", "--no-background"],
                    &ids,
                    &mut observations,
                )?;
                let sessions = successful(
                    executable,
                    &fixture,
                    &["--json", "sessions"],
                    &ids,
                    &mut observations,
                )?;
                assert_eq!(sessions.as_array().unwrap().len(), 3);
                successful(
                    executable,
                    &fixture,
                    &["--json", "usage", "--by-model", "--timezone", "UTC"],
                    &ids,
                    &mut observations,
                )?;
                let rows = successful(
                    executable,
                    &fixture,
                    &[
                        "--json",
                        "sql",
                        "SELECT message_id,session_id,message_no,session_no,role,text FROM history_messages ORDER BY message_id",
                        "--full",
                    ],
                    &ids,
                    &mut observations,
                )?;
                let current = rows["rows"].as_array().unwrap();
                if let Some(previous) = retained.as_ref() {
                    for message in previous {
                        assert!(current.contains(message), "retained public row changed");
                    }
                }
                retained = Some(current.clone());
            }
            "append" => fixture.append(
                step["profile"].as_str().unwrap(),
                step["input"].as_str().unwrap(),
            )?,
            "reopen" => {} // Every CLI command already closes and reopens the store.
            "remove_sources" => {
                for p in fixture.manifest["profiles"].as_array().unwrap() {
                    std::fs::remove_dir_all(fixture.root().join(p["root"].as_str().unwrap()))?;
                }
            }
            "reprocess" => {
                successful(
                    executable,
                    &fixture,
                    &["--json", "reprocess", "--dry-run"],
                    &ids,
                    &mut observations,
                )?;
                successful(
                    executable,
                    &fixture,
                    &["--json", "reprocess"],
                    &ids,
                    &mut observations,
                )?;
                let rows = successful(
                    executable,
                    &fixture,
                    &[
                        "--json",
                        "sql",
                        "SELECT message_id,session_id,message_no,session_no,role,text FROM history_messages ORDER BY message_id",
                        "--full",
                    ],
                    &ids,
                    &mut observations,
                )?;
                assert_eq!(rows["rows"], json!(retained));
            }
            other => anyhow::bail!("unsupported scenario step {other}"),
        }
    }
    successful(
        executable,
        &fixture,
        &["--json", "status"],
        &ids,
        &mut observations,
    )?;
    successful(
        executable,
        &fixture,
        &["--json", "projects"],
        &ids,
        &mut observations,
    )?;
    let tools = successful(
        executable,
        &fixture,
        &[
            "--json",
            "sql",
            "SELECT native_call_id,tool_name,arguments_json,output_preview,has_result FROM history_tool_activity ORDER BY native_call_id",
            "--full",
        ],
        &ids,
        &mut observations,
    )?;
    assert_eq!(tools["rows"], fixture.expected["tools"]);
    let hits = successful(
        executable,
        &fixture,
        &[
            "--json",
            "search",
            "repair lunar cache",
            "--mode",
            "exact",
            "--profile",
            "personal",
            "--no-refresh",
            "--no-background",
        ],
        &ids,
        &mut observations,
    )?;
    assert_eq!(hits["results"].as_array().unwrap().len(), 1);
    assert_eq!(hits["results"][0].as_object().unwrap().len(), 7);
    let id = hits["results"][0]["id"].as_i64().unwrap().to_string();
    successful(
        executable,
        &fixture,
        &["--json", "show", &id, "--before", "1", "--after", "1"],
        &ids,
        &mut observations,
    )?;
    successful(
        executable,
        &fixture,
        &[
            "--json",
            "search",
            "repair",
            "--profile",
            "personal",
            "--no-refresh",
            "--no-background",
            "--explain",
        ],
        &ids,
        &mut observations,
    )?;
    successful(executable, &fixture, &["sessions"], &ids, &mut observations)?;
    successful(
        executable,
        &fixture,
        &["--json", "semantic", "index"],
        &ids,
        &mut observations,
    )?;
    successful(
        executable,
        &fixture,
        &["--json", "raw", "verify"],
        &ids,
        &mut observations,
    )?;
    successful(
        executable,
        &fixture,
        &["--json", "raw", "stats"],
        &ids,
        &mut observations,
    )?;
    successful(
        executable,
        &fixture,
        &["--json", "raw", "sources"],
        &ids,
        &mut observations,
    )?;
    let exported = run(
        executable,
        &fixture,
        &[
            "raw",
            "export",
            "shared-native",
            "--profile",
            "personal",
            "--stdout",
        ],
    )?;
    ensure!(exported.status.success(), "raw export failed: {exported:?}");
    assert_eq!(
        exported.stdout,
        std::fs::read(fixture.inputs.join("personal.jsonl"))?
    );
    let error = observe(
        executable,
        &fixture,
        &["--json", "show", &id, "--profile", "work"],
        &ids,
        &mut observations,
    )?;
    assert_eq!(error["exit"], 1);
    let error = observe(
        executable,
        &fixture,
        &[
            "search",
            "x",
            "--mode",
            "exact",
            "--rerank",
            "--no-refresh",
            "--no-background",
        ],
        &ids,
        &mut observations,
    )?;
    assert_eq!(error["exit"], 1);
    // The second search bails out after the first reconciles missing inputs.
    successful(
        executable,
        &fixture,
        &["--json", "search", "lunar", "--no-background", "--explain"],
        &ids,
        &mut observations,
    )?;
    successful(
        executable,
        &fixture,
        &["--json", "search", "lunar", "--no-background", "--explain"],
        &ids,
        &mut observations,
    )?;
    Ok(observations)
}

#[test]
fn cli_observes_the_public_fixture_and_optionally_matches_a_preserved_executable() -> Result<()> {
    let current = scenario(Path::new(env!("CARGO_BIN_EXE_llm-history")))?;
    if let Some(baseline) = std::env::var_os("LLM_HISTORY_BASELINE") {
        let previous = scenario(Path::new(&baseline))?;
        assert_eq!(current.len(), previous.len());
        for (candidate, baseline) in current.iter().zip(&previous) {
            assert_eq!(candidate, baseline, "CLI contract difference");
        }
        eprintln!(
            "{} CLI observations match the preserved executable",
            current.len()
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn replay_reports_why_preparation_failed_and_keeps_committed_messages() -> Result<()> {
    let fixture = Fixture::new()?;
    let exe = Path::new(env!("CARGO_BIN_EXE_llm-history"));
    let root = fixture
        .root()
        .join(fixture.profile("personal")["root"].as_str().unwrap());
    ensure!(
        run(
            exe,
            &fixture,
            &[
                "profile",
                "add",
                "personal",
                "--harness",
                "codex",
                "--root",
                root.to_str().unwrap(),
            ]
        )?
        .status
        .success(),
        "register synthetic profile"
    );
    ensure!(
        run(exe, &fixture, &["update", "--no-background"])?
            .status
            .success(),
        "capture synthetic history"
    );
    let before = run(exe, &fixture, &["--json", "sessions"])?;
    assert!(before.status.success());
    let failed = command(exe, &fixture, &["--json", "reprocess"])
        .env("TMPDIR", fixture.root().join("nonexistent-temp-directory"))
        .output()?;
    assert!(!failed.status.success());
    let receipt: Value = serde_json::from_slice(&failed.stdout)?;
    let cause = receipt["errors"][0].as_str().unwrap();
    assert!(cause.contains("replay preparation failed"), "{cause}");
    assert!(cause.contains("No such file or directory"), "{cause}");
    let after = run(exe, &fixture, &["--json", "sessions"])?;
    assert!(after.status.success());
    assert_eq!(before.stdout, after.stdout);
    Ok(())
}

#[test]
fn foreground_backfill_honors_the_database_memory_setting() -> Result<()> {
    let fixture = Fixture::new()?;
    let exe = Path::new(env!("CARGO_BIN_EXE_llm-history"));
    ensure!(
        run(exe, &fixture, &["init"])?.status.success(),
        "initialize an empty archive"
    );
    let attempt = run(
        exe,
        &fixture,
        &[
            "--memory-mib",
            "0",
            "semantic",
            "backfill",
            "--foreground",
            "--endpoint",
            "http://127.0.0.1:1",
            "--seconds",
            "2",
        ],
    )?;
    assert!(!attempt.status.success());
    let error = String::from_utf8(attempt.stderr)?;
    assert!(
        error.contains("memory and batch limits must be positive"),
        "{error}"
    );
    Ok(())
}

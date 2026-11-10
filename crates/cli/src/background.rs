//! Search-time refresh and the detached background embedding worker.
use super::*;
#[cfg(test)]
use llm_history_store::Store;

pub(crate) fn refresh(
    db: &Path,
    _cache: &Path,
    names: &[String],
    source: Option<&str>,
    force: bool,
) -> Result<Value> {
    Ok(serde_json::to_value(
        history(db).refresh(names, source, force)?,
    )?)
}
pub(crate) fn automatic_options(
    db: &Path,
    cache: &Path,
    names: &[String],
    source: Option<&str>,
    inference: &Inference,
) -> Result<Option<Backfill>> {
    let queue = inference_queue(db, inference)?;
    if launch_blocked(&queue, true)?.is_some() {
        return Ok(None);
    }
    if inference.endpoint.is_some() {
        if !inference.background_remote {
            return Ok(None);
        }
    } else if llm_history_models::native_server(cache).is_err()
        || llm_history_models::artifact_path(&llm_history_models::model_artifacts()[0], false)
            .is_err()
    {
        return Ok(None);
    }
    let mut options = HistoryOptions::new(db);
    options.storage = store_options();
    options.lock_timeout = Duration::ZERO;
    let store = History::with_options(options);
    let profiles = store
        .profiles(names, false)?
        .into_iter()
        .filter(|p| {
            (!names.is_empty() || p.enabled)
                && source.is_none_or(|s| s == "all" || s == p.harness.key())
        })
        .map(|p| p.name)
        .collect::<Vec<_>>();
    if profiles.is_empty() {
        return Ok(None);
    }
    let model = profile_for(inference)?;
    if !store.has_pending_messages(&model.id, &profiles, source)?
        && !queue.read_dir().ok().is_some_and(|entries| {
            entries.flatten().any(|e| {
                e.path().extension().is_some_and(|x| x == "json")
                    && !matches!(e.file_name().to_str(), Some("status.json" | "launch.json"))
            })
        })
    {
        return Ok(None);
    }
    Ok(Some(Backfill {
        foreground: false,
        seconds: 300.,
        max_documents: 10000,
        max_tokens: 2_000_000,
        queue_mib: 64,
        queue_items: 1024,
        max_document_chars: 1_000_000,
        read_batch: 64,
        retry_failures: false,
        source: source.map(str::to_owned),
        profile: profiles,
        inference: inference.clone(),
    }))
}
pub(crate) fn maybe_background(
    db: &Path,
    cache: &Path,
    profiles: &[String],
    source: Option<&str>,
    inference: &Inference,
) {
    if std::env::var("LLM_HISTORY_RS_BACKGROUND")
        .is_ok_and(|s| matches!(s.to_lowercase().as_str(), "off" | "0" | "false"))
    {
        return;
    }
    if let Ok(Some(options)) = automatic_options(db, cache, profiles, source, inference) {
        let _ = launch(db, cache, &options, true);
    }
}
pub(crate) fn launch_blocked(queue: &Path, automatic: bool) -> Result<Option<Value>> {
    if !queue.is_dir() {
        return Ok(None);
    }
    let runtime = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(queue.join("runtime.lock"))?;
    match fs2::FileExt::try_lock_exclusive(&runtime) {
        Ok(()) => {}
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
        {
            return Ok(Some(json!({"status":"already_running"})));
        }
        Err(e) => return Err(e.into()),
    }
    drop(runtime);
    let status = std::fs::read_to_string(queue.join("status.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or(Value::Null);
    let now = chrono::Utc::now().timestamp();
    if status["status"] == "starting" && now - status["updated_at"].as_i64().unwrap_or(0) < 60 {
        return Ok(Some(json!({"status":"starting"})));
    }
    let fallback = status["updated_at"].as_i64().unwrap_or(0)
        + if matches!(
            status["status"].as_str(),
            Some("failed" | "repeated_failures" | "transient_failure" | "input_limit")
        ) {
            1800
        } else {
            30
        };
    if automatic
        && status["status"] != "caught_up"
        && now < status["next_launch_at"].as_i64().unwrap_or(fallback)
    {
        return Ok(Some(json!({"status":"cooldown"})));
    }
    Ok(None)
}
pub(crate) fn validate_backfill(b: &Backfill) -> Result<()> {
    if !b.seconds.is_finite()
        || b.seconds <= 0.
        || b.max_documents == 0
        || b.max_tokens == 0
        || b.queue_mib == 0
        || b.queue_mib > 4096
        || b.queue_items == 0
        || b.max_document_chars == 0
        || !(1..=256).contains(&b.read_batch)
    {
        bail!("worker budgets must be positive; queue MiB <=4096 and read batch 1..=256");
    }
    Ok(())
}
pub(crate) fn launch(db: &Path, cache: &Path, b: &Backfill, automatic: bool) -> Result<Value> {
    let queue = inference_queue(db, &b.inference)?;
    std::fs::create_dir_all(&queue)?;
    let launch_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(queue.join("launch.lock"))?;
    fs2::FileExt::try_lock_exclusive(&launch_lock).context("worker launch already in progress")?;
    validate_backfill(b)?;
    if let Some(status) = launch_blocked(&queue, automatic)? {
        return Ok(status);
    }
    llm_history_search::status_json(
        &queue.join("status.json"),
        &json!({"status":"starting","updated_at":chrono::Utc::now().timestamp(),"model_profile":profile_for(&b.inference)?.id,"profiles":b.profile}),
    )?;
    let executable = std::env::current_exe()?;
    let mut cmd = worker_command(&executable, db, cache, b, store_options());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Only the async-signal-safe setsid syscall runs between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                unsafe extern "C" {
                    fn setsid() -> i32;
                }
                if setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000208);
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(queue.join("worker.log"))?;
    let child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    Ok(
        json!({"status":"launched","pid":child.id(),"timestamp":chrono::Utc::now().timestamp(),"seconds":b.seconds}),
    )
}

fn worker_command(
    executable: &Path,
    db: &Path,
    cache: &Path,
    b: &Backfill,
    storage: llm_history_types::storage::StoreOptions,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(executable);
    cmd.arg("--db").arg(db).arg("--cache").arg(cache).args([
        "--memory-mib",
        &storage.memory_mib.to_string(),
        "--batch-records",
        &storage.batch_records.to_string(),
        "--batch-mib",
        &(storage.batch_bytes / (1024 * 1024)).to_string(),
        "semantic",
        "backfill",
        "--foreground",
        "--seconds",
        &b.seconds.to_string(),
        "--max-documents",
        &b.max_documents.to_string(),
        "--max-tokens",
        &b.max_tokens.to_string(),
        "--queue-mib",
        &b.queue_mib.to_string(),
        "--queue-items",
        &b.queue_items.to_string(),
        "--max-document-chars",
        &b.max_document_chars.to_string(),
        "--read-batch",
        &b.read_batch.to_string(),
        "--dimensions",
        &b.inference.dimensions.to_string(),
        "--timeout-seconds",
        &b.inference.timeout_seconds.to_string(),
    ]);
    if let Some(source) = &b.source {
        cmd.arg("--source").arg(source);
    }
    for p in &b.profile {
        cmd.arg("--profile").arg(p);
    }
    if b.retry_failures {
        cmd.arg("--retry-failures");
    }
    for (flag, value) in [
        ("--endpoint", b.inference.endpoint.as_deref()),
        ("--model-name", b.inference.model_name.as_deref()),
    ] {
        if let Some(value) = value {
            cmd.arg(flag).arg(value);
        }
    }
    if let Some(path) = &b.inference.api_key_file {
        cmd.arg("--api-key-file").arg(path);
    }
    cmd
}

#[cfg(test)]
mod worker_options_tests {
    use super::*;

    #[test]
    fn only_failure_or_active_work_can_block_an_immediate_restart_after_catchup() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let queue = dir.path();
        let next = chrono::Utc::now().timestamp() + 300;
        let status = |state| {
            std::fs::write(
                queue.join("status.json"),
                json!({"status":state,"next_launch_at":next}).to_string(),
            )
        };
        // A receipt left by an older binary can still contain the old success delay.
        status("caught_up")?;
        assert!(launch_blocked(queue, true)?.is_none());
        status("transient_failure")?;
        assert_eq!(
            launch_blocked(queue, true)?,
            Some(json!({"status":"cooldown"}))
        );
        Ok(())
    }

    #[test]
    fn automatic_worker_preserves_scope_model_dimensions_and_credentials() -> Result<()> {
        let d = tempfile::tempdir()?;
        let db = d.path().join("history.duckdb");
        let store = Store::open(&db, false)?;
        for name in ["personal", "work"] {
            let root = d.path().join(name);
            std::fs::create_dir_all(root.join("sessions"))?;
            store.add_profile(name, Harness::Codex, &root, None, None)?;
            std::fs::write(
                root.join("sessions/a.jsonl"),
                llm_history_core::fixture_record(Harness::Codex, "same-native", name)
                    .iter()
                    .map(|r| format!("{r}\n"))
                    .collect::<String>(),
            )?;
        }
        drop(store);
        llm_history_app::History::new(&db)
            .update(&llm_history_app::api::ingestion::UpdateRequest::default())?;
        let inference = Inference {
            endpoint: Some("http://127.0.0.1:9".into()),
            model_name: Some("work-model".into()),
            api_key_file: Some(d.path().join("credentials")),
            background_remote: true,
            dimensions: 256,
            ..Default::default()
        };
        let options =
            automatic_options(&db, d.path(), &["work".into()], Some("codex"), &inference)?.unwrap();
        let command = worker_command(
            Path::new("llm-history"),
            &db,
            d.path(),
            &options,
            llm_history_types::storage::StoreOptions {
                memory_mib: 73,
                batch_records: 511,
                batch_bytes: 6 * 1024 * 1024,
            },
        );
        let child =
            App::try_parse_from(std::iter::once(command.get_program()).chain(command.get_args()))?;
        assert_eq!(
            (child.memory_mib, child.batch_records, child.batch_mib),
            (73, 511, 6)
        );
        assert_eq!(options.profile, vec!["work"]);
        assert_eq!(options.source.as_deref(), Some("codex"));
        assert_eq!(options.inference.dimensions, 256);
        assert_eq!(options.inference.model_name, inference.model_name);
        assert_eq!(options.inference.api_key_file, inference.api_key_file);
        assert_eq!(options.inference.endpoint, inference.endpoint);
        assert!(
            automatic_options(
                &db,
                d.path(),
                &[],
                None,
                &Inference {
                    background_remote: false,
                    ..inference.clone()
                }
            )?
            .is_none()
        );
        assert!(
            automatic_options(&db, d.path(), &["work".into()], Some("claude"), &inference)?
                .is_none()
        );
        // Scheduling checks must finish without opening a busy database.
        let _writer = Store::open(&db, false)?;
        let queue = inference_queue(&db, &inference)?;
        std::fs::create_dir_all(&queue)?;
        std::fs::write(
            queue.join("status.json"),
            json!({
                "next_launch_at": chrono::Utc::now().timestamp() + 300
            })
            .to_string(),
        )?;
        assert!(automatic_options(&db, d.path(), &[], None, &inference)?.is_none());
        std::fs::remove_file(queue.join("status.json"))?;
        let runtime = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(queue.join("runtime.lock"))?;
        fs2::FileExt::try_lock_exclusive(&runtime)?;
        assert!(automatic_options(&db, d.path(), &[], None, &inference)?.is_none());
        Ok(())
    }
}
#[cfg(all(test, unix))]
mod refresh_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn a_failing_source_still_starts_the_refresh_cooldown() -> Result<()> {
        let d = tempfile::tempdir()?;
        let db = d.path().join("history.duckdb");
        let root = d.path().join("codex");
        std::fs::create_dir_all(root.join("sessions"))?;
        let bad = root.join("sessions/unreadable.jsonl");
        std::fs::write(&bad, "{}\n")?;
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000))?;
        let store = Store::open(&db, false)?;
        store.add_profile("personal", Harness::Codex, &root, None, None)?;
        drop(store);
        let cache = d.path().join("cache");
        let first = refresh(&db, &cache, &[], None, false)?;
        assert_eq!(first.as_array().map(Vec::len), Some(1));
        assert!(!first[0]["errors"].as_array().unwrap().is_empty());
        // Errors are reported once; later searches within five minutes skip the update.
        assert_eq!(refresh(&db, &cache, &[], None, false)?, json!([]));
        Ok(())
    }
}

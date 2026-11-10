//! Lexical index maintenance, embedding coverage and the embedding worker.
use super::*;
use llm_history_search::WorkBudget;

pub(crate) fn semantic(ctx: &Ctx, command: Semantic) -> Result<()> {
    match command {
        Semantic::Search(_) => unreachable!(),
        Semantic::Index { force } => {
            emit(
                &serde_json::to_value(ctx.history().index(force)?)?,
                ctx.json,
            )?;
        }
        Semantic::Status { inference, profile } => {
            let s = ctx.history();
            let p = profile_for(&inference)?;
            let mut coverage = s.coverage(&p.id, &profile)?;
            coverage["profile"] = serde_json::to_value(p)?;
            emit(&coverage, ctx.json)?;
        }
        Semantic::BackfillStatus { inference } => {
            let queue = inference_queue(&ctx.db, &inference)?;
            let path = queue.join("status.json");
            let mut status = if path.exists() {
                serde_json::from_str::<Value>(&std::fs::read_to_string(&path)?)?
            } else {
                json!({"status":"idle"})
            };
            if path.exists() {
                let lock = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(queue.join("runtime.lock"))?;
                let running = match fs2::FileExt::try_lock_exclusive(&lock) {
                    Ok(()) => false,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                    {
                        true
                    }
                    Err(e) => return Err(e.into()),
                };
                status["running"] = json!(running);
                if !running
                    && (matches!(
                        status["status"].as_str(),
                        Some("loading" | "running" | "waiting_for_database")
                    ) || status["status"] == "starting"
                        && chrono::Utc::now().timestamp()
                            - status["updated_at"].as_i64().unwrap_or(0)
                            >= 60)
                {
                    status["status"] = json!("interrupted");
                }
            }
            emit(&status, ctx.json)?;
        }
        Semantic::Backfill(b) if !b.foreground => {
            emit(&launch(&ctx.db, &ctx.cache, &b, false)?, ctx.json)?
        }
        Semantic::Backfill(b) => backfill(ctx, b)?,
    }
    Ok(())
}

pub(crate) fn backfill(ctx: &Ctx, b: Backfill) -> Result<()> {
    let queue = inference_queue(&ctx.db, &b.inference)?;
    std::fs::create_dir_all(&queue)?;
    let runtime_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(queue.join("runtime.lock"))?;
    fs2::FileExt::try_lock_exclusive(&runtime_lock).context("worker runtime already running")?;
    validate_backfill(&b)?;
    let started = std::time::Instant::now();
    llm_history_search::status_json(
        &queue.join("status.json"),
        &json!({"status":"loading","pid":std::process::id()}),
    )?;
    let result = (|| -> Result<llm_history_types::jobs::WorkReceipt> {
        let mut inference = b.inference.clone();
        inference.timeout_seconds = inference
            .timeout_seconds
            .min(b.seconds.ceil() as u64)
            .max(1);
        let mut e = engine(&ctx.cache, &inference, false)?;
        let remaining = b.seconds - started.elapsed().as_secs_f64();
        if remaining <= 0. {
            return Ok(llm_history_types::jobs::WorkReceipt {
                status: "budget_exhausted".into(),
                elapsed_seconds: started.elapsed().as_secs_f64(),
                ..Default::default()
            });
        }
        ctx.history().backfill(
            &queue,
            e.embedding(),
            &WorkBudget {
                seconds: remaining,
                documents: b.max_documents,
                tokens: b.max_tokens,
                queue_bytes: b.queue_mib * 1024 * 1024,
                retry_failures: b.retry_failures,
                profiles: b.profile,
                source: b.source,
                queue_items: b.queue_items,
                max_document_chars: b.max_document_chars,
                read_batch: b.read_batch,
                cancellation: cancellation(),
            },
        )
    })();
    match result {
        Ok(mut receipt) => {
            receipt.elapsed_seconds = started.elapsed().as_secs_f64();
            if cancellation().is_cancelled() {
                receipt.status = "cancelled".into();
            }
            llm_history_search::status_json(&queue.join("status.json"), &receipt)?;
            emit(&serde_json::to_value(&receipt)?, ctx.json)?;
            if matches!(
                receipt.status.as_str(),
                "failed"
                    | "transient_failure"
                    | "repeated_failures"
                    | "cancelled"
                    | "input_limit"
                    | "database_replaced"
            ) {
                bail!("embedding work ended with {}", receipt.status);
            }
        }
        Err(error) => {
            llm_history_search::status_json(
                &queue.join("status.json"),
                &json!({"status":if cancellation().is_cancelled(){"cancelled"}else{"failed"},"error_type":"inference_or_storage","updated_at":chrono::Utc::now().timestamp(),"elapsed_seconds":started.elapsed().as_secs_f64()}),
            )?;
            return Err(error);
        }
    }
    Ok(())
}

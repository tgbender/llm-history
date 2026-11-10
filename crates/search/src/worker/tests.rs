//! Worker tests: staging, merging, budgets and failure handling.
use super::*;
const SQL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(llm_history_types::storage::DEFAULT_SQL_TIMEOUT_SECONDS);
use crate::{Mode, SearchRequest, search};
use llm_history_models::{ModelProfile, TokenCounter};
use llm_history_types::query::Filter;
use serde_json::json;

fn publish_fixture_vectors(
    store: &Store,
    model: &ModelProfile,
    messages: &[llm_history_types::query::Message],
    vectors: &[&[f32]],
) -> Result<()> {
    assert_eq!(messages.len(), vectors.len());
    let docs = messages
        .iter()
        .zip(vectors)
        .map(|(message, vector)| {
            Ok(EmbeddingDocument {
                id: message.id.parse()?,
                text_hash: message.text_hash.clone(),
                vectors: vec![vector.to_vec()],
                failure: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(
        store.publish_embedding_batch(
            &model.id,
            &serde_json::to_string(model)?,
            &EmbeddingBatch::new(&docs)?
        )?,
        messages.len()
    );
    Ok(())
}

fn merge_queue_in(
    store: &Store,
    directory: &Path,
    scope: Option<(&str, &WorkBudget)>,
) -> Result<usize> {
    let default = WorkBudget::default();
    let budget = scope.map(|(_, b)| b).unwrap_or(&default);
    let mut queue = StageQueue::load(
        directory,
        budget,
        Instant::now() + std::time::Duration::from_secs(300),
        &mut WorkReceipt::default(),
    )?
    .unwrap();
    let Some(first) = queue.files.iter().flat_map(|f| &f.entries).next() else {
        return Ok(0);
    };
    let model = scope.map(|(id, _)| id).unwrap_or(&first.profile).to_owned();
    let review = queue.review(store, &store.identity()?, &model, budget)?;
    let mut receipt = WorkReceipt::default();
    queue.acknowledge(&review.acknowledged, &mut receipt)?;
    if review.documents.is_empty() {
        return Ok(0);
    }
    let contract = store.model_contract(&model)?;
    let prepared = EmbeddingBatch::new(&review.documents)?;
    let count = store.publish_embedding_batch(&model, &contract, &prepared)?;
    queue.acknowledge(&review.selected, &mut receipt)?;
    Ok(count)
}

struct Fake {
    profile: ModelProfile,
    db: std::path::PathBuf,
    fail: bool,
    calls: usize,
    transient: bool,
    rejected: bool,
    rejected_tokens: bool,
    read_probe: bool,
}
impl TokenCounter for Fake {
    fn token_count(&mut self, s: &str) -> Result<usize> {
        if self.rejected_tokens {
            return Err(InferenceError::Http(401).into());
        }
        Ok(s.chars().count())
    }
}
impl Embedder for Fake {
    fn profile(&self) -> &ModelProfile {
        &self.profile
    }
    fn embed(&mut self, texts: &[String], _: bool) -> Result<Vec<Vec<f32>>> {
        // Counts embedded texts, so batching cannot hide repeated inference.
        self.calls += texts.len();
        let _s = Store::open(&self.db, self.read_probe)?;
        if self.transient {
            return Err(InferenceError::Http(503).into());
        }
        if self.rejected {
            return Err(InferenceError::Http(401).into());
        }
        if self.fail {
            bail!("private exception text");
        }
        Ok(texts.iter().map(|_| vec![1., 0.]).collect())
    }
}
fn setup() -> Result<(tempfile::TempDir, std::path::PathBuf)> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("test.duckdb");
    let root = d.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let s = Store::open(&db, false)?;
    let p = s.add_profile(
        "personal",
        llm_history_types::Harness::Codex,
        &root,
        None,
        None,
    )?;
    let records = llm_history_core::fixture_record(p.harness, "thread", "opaque Rust handle");
    std::fs::write(
        root.join("sessions/a.jsonl"),
        records.iter().map(|x| format!("{x}\n")).collect::<String>(),
    )?;
    crate::fixtures::capture(&s, &[], false)?;
    drop(s);
    Ok((d, db))
}
fn fake(db: &Path) -> Fake {
    let mut p = llm_history_models::gemma_profile(768).unwrap();
    p.dimensions = 2;
    p.max_tokens = 50;
    p.document_prefix = String::new();
    Fake {
        profile: p,
        db: db.to_owned(),
        fail: false,
        calls: 0,
        transient: false,
        rejected: false,
        rejected_tokens: false,
        read_probe: false,
    }
}
#[test]
fn inference_holds_no_database_lock_and_is_resumable() -> Result<()> {
    let (d, db) = setup()?;
    let mut f = fake(&db);
    let q = d.path().join("queue");
    atomic_json(
        &q.join("launch.json"),
        &json!({"status":"launched","pid":123}),
    )?;
    atomic_json(&q.join("status.json"), &json!({"status":"loading"}))?;
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default()
        )?
        .merged,
        1
    );
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default()
        )?
        .merged,
        0
    );
    Ok(())
}
#[test]
fn poison_failure_is_sanitized_and_skipped() -> Result<()> {
    let (d, db) = setup()?;
    let mut f = fake(&db);
    f.fail = true;
    let q = d.path().join("queue");
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default()
        )?
        .failures,
        1
    );
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default()
        )?
        .failures,
        0
    );
    let s = Store::open(&db, true)?;
    assert!(
        !s.sql_bounded(
            "select * from embedding_failures",
            10,
            1000,
            100_000,
            false,
            SQL_TIMEOUT
        )?
        .to_string()
        .contains("private")
    );
    Ok(())
}
#[test]
fn semantic_requires_explicit_engine() -> Result<()> {
    let (_d, db) = setup()?;
    let s = Store::open(&db, true)?;
    let r = SearchRequest {
        query: "opaque".into(),
        mode: Mode::Semantic,
        ..SearchRequest::default()
    };
    assert!(search(|| Ok(&s), &r, None, None).is_err());
    Ok(())
}
#[test]
fn lexical_filters_and_budget() -> Result<()> {
    let (_d, db) = setup()?;
    let s = Store::open(&db, true)?;
    let r = SearchRequest {
        query: "opaque".into(),
        max_output_chars: 12,
        ..SearchRequest::default()
    };
    let out = search(|| Ok(&s), &r, None, None)?;
    assert_eq!(out.results.len(), 1);
    assert!(out.results[0].message.text.chars().count() <= 12);
    Ok(())
}
#[test]
fn partial_chunks_resume_without_repeating_inference() -> Result<()> {
    let (d, db) = setup()?;
    let mut f = fake(&db);
    f.profile.max_tokens = 8;
    let q = d.path().join("queue");
    let budget = WorkBudget {
        tokens: 8,
        ..WorkBudget::default()
    };
    let first = backfill(&db, StoreOptions::default(), &q, &mut f, &budget)?;
    assert_eq!(first.status, "budget_exhausted");
    assert_eq!(first.merged, 0);
    let calls = f.calls;
    assert!(calls > 0);
    let message = {
        let s = Store::open(&db, true)?;
        s.messages_page(&Filter::default(), None, usize::MAX)?
            .remove(0)
    };
    let total = split_for_tokens(&mut f, &message.text, "", 8)?.len();
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default()
        )?
        .merged,
        1
    );
    assert_eq!(f.calls, total);
    Ok(())
}
#[test]
fn commit_before_ack_replays_idempotently() -> Result<()> {
    let (d, db) = setup()?;
    let q = d.path().join("queue");
    std::fs::create_dir_all(&q)?;
    let f = fake(&db);
    let s = Store::open(&db, false)?;
    let message = s
        .messages_page(&Filter::default(), None, usize::MAX)?
        .remove(0);
    s.register_model_profile(&f.profile.id, &serde_json::to_string(&f.profile)?)?;
    publish_fixture_vectors(&s, &f.profile, std::slice::from_ref(&message), &[&[1., 0.]])?;
    let stage = Staged {
        database: s.identity()?,
        profile: f.profile.id.clone(),
        message: message.clone(),
        chunk: 0,
        total: 1,
        vector: Some(vec![1., 0.]),
        failure: None,
    };
    atomic_json(&q.join("replay.json"), &stage)?;
    assert_eq!(merge_queue_in(&s, &q, None)?, 0);
    assert_eq!(merge_queue_in(&s, &q, None)?, 0);
    assert_eq!(s.embedding_vectors(&f.profile.id, &message)?.len(), 1);
    Ok(())
}
#[test]
fn stale_stages_and_database_identity_are_rejected() -> Result<()> {
    let (d, db) = setup()?;
    let q = d.path().join("queue");
    std::fs::create_dir_all(&q)?;
    let f = fake(&db);
    let s = Store::open(&db, false)?;
    let message = s
        .messages_page(&Filter::default(), None, usize::MAX)?
        .remove(0);
    let stage = Staged {
        database: "different-database".into(),
        profile: f.profile.id,
        message,
        chunk: 0,
        total: 1,
        vector: Some(vec![1., 0.]),
        failure: None,
    };
    atomic_json(&q.join("stale.json"), &stage)?;
    assert_eq!(merge_queue_in(&s, &q, None)?, 0);
    assert!(!q.join("stale.json").exists());
    Ok(())
}
#[test]
fn transient_errors_do_not_poison_documents() -> Result<()> {
    let (d, db) = setup()?;
    let mut f = fake(&db);
    f.transient = true;
    let q = d.path().join("queue");
    let receipt = backfill(
        &db,
        StoreOptions::default(),
        &q,
        &mut f,
        &WorkBudget::default(),
    )?;
    assert_eq!(receipt.status, "transient_failure");
    assert_eq!(receipt.failures, 0);
    f.transient = false;
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default()
        )?
        .merged,
        1
    );
    Ok(())
}
#[test]
fn rejected_credentials_fail_the_run_without_poisoning_documents() -> Result<()> {
    let (d, db) = setup()?;
    let mut f = fake(&db);
    let q = d.path().join("queue");
    for tokens in [true, false] {
        (f.rejected, f.rejected_tokens) = (!tokens, tokens);
        let error = backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default(),
        )
        .unwrap_err();
        assert!(
            matches!(error.downcast_ref(), Some(InferenceError::Http(401))),
            "{error:#}"
        );
    }
    (f.rejected, f.rejected_tokens) = (false, false);
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default()
        )?
        .merged,
        1
    );
    Ok(())
}
#[test]
fn queue_and_worker_locks_bound_work() -> Result<()> {
    let (d, db) = setup()?;
    let mut f = fake(&db);
    let q = d.path().join("queue");
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget {
                queue_bytes: 1,
                ..WorkBudget::default()
            }
        )?
        .status,
        "budget_exhausted"
    );
    assert_eq!(f.calls, 0);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(q.join("worker.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)?;
    assert!(
        backfill(
            &db,
            StoreOptions::default(),
            &q,
            &mut f,
            &WorkBudget::default()
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn restart_enforces_aggregate_byte_and_item_budgets_and_preserves_staging() -> Result<()> {
    let (d, db) = setup()?;
    let store = Store::open(&db, true)?;
    let mut message = store.messages_page(&Filter::default(), None, 1)?.remove(0);
    message.text.clear();
    let stage = Staged {
        database: store.identity()?,
        profile: fake(&db).profile.id,
        message,
        chunk: 0,
        total: 1,
        vector: Some(vec![1., 0.]),
        failure: None,
    };
    drop(store);
    // Exercise multiple files and a single batch containing multiple entries.
    for batch in [false, true] {
        for bytes_limit in [false, true] {
            let dir = d.path().join(format!("restart-{batch}-{bytes_limit}"));
            std::fs::create_dir_all(&dir)?;
            let contents = if batch {
                vec![serde_json::to_vec(&vec![stage.clone(), stage.clone()])?]
            } else {
                vec![
                    serde_json::to_vec(&stage)?,
                    serde_json::to_vec(&vec![stage.clone()])?,
                ]
            };
            for (i, data) in contents.iter().enumerate() {
                std::fs::write(dir.join(format!("{i}.json")), data)?;
            }
            let mut engine = fake(&db);
            let budget = if bytes_limit {
                WorkBudget {
                    queue_bytes: contents.iter().map(|s| s.len() as u64).sum::<u64>() - 1,
                    ..Default::default()
                }
            } else {
                WorkBudget {
                    queue_items: 1,
                    ..Default::default()
                }
            };
            let error =
                backfill(&db, StoreOptions::default(), &dir, &mut engine, &budget).unwrap_err();
            let limit = if bytes_limit {
                "queue_bytes"
            } else {
                "queue_items"
            };
            assert!(format!("{error:#}").contains(limit), "{error:#}");
            assert_eq!(engine.calls, 0);
            for (i, data) in contents.iter().enumerate() {
                assert_eq!(&std::fs::read(dir.join(format!("{i}.json")))?, data);
            }
            // Raising the configured bound loads the same durable work without inference.
            let resumed = backfill(
                &db,
                StoreOptions::default(),
                &dir,
                &mut engine,
                &WorkBudget::default(),
            )?;
            assert_eq!(resumed.status, "caught_up");
            assert_eq!(resumed.queued_chunks, 0);
            assert_eq!(engine.calls, 0);
        }
    }
    Ok(())
}

#[test]
fn batch_item_budget_is_checked_before_decoding_the_next_entry() -> Result<()> {
    let (d, db) = setup()?;
    let store = Store::open(&db, true)?;
    let stage = Staged {
        database: store.identity()?,
        profile: fake(&db).profile.id,
        message: store.messages_page(&Filter::default(), None, 1)?.remove(0),
        chunk: 0,
        total: 1,
        vector: Some(vec![1., 0.]),
        failure: None,
    };
    drop(store);
    let directory = d.path().join("bounded-batch");
    std::fs::create_dir_all(&directory)?;
    // An invalid next entry must be rejected by the item limit before its parser runs.
    let data = format!("[{},{{\"message\":", serde_json::to_string(&stage)?);
    std::fs::write(directory.join("batch.json"), &data)?;
    let error = backfill(
        &db,
        StoreOptions::default(),
        &directory,
        &mut fake(&db),
        &WorkBudget {
            queue_items: 1,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("queue_items"), "{error:#}");
    assert_eq!(std::fs::read_to_string(directory.join("batch.json"))?, data);
    Ok(())
}

#[test]
fn queue_acquisition_observes_cancellation_deadlines_and_growing_files() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("stage.json");
    std::fs::write(&path, b"123456")?;
    let budget = WorkBudget::default();
    let deadline = Instant::now() + std::time::Duration::from_secs(300);
    let mut reader = QueueReader {
        file: File::open(&path)?,
        remaining: 5,
        bytes: 0,
        budget: &budget,
        deadline,
    };
    let mut data = Vec::new();
    let error = reader.read_to_end(&mut data).unwrap_err();
    assert!(error.to_string().contains("queue_bytes"));
    assert_eq!(data, b"12345");
    assert_eq!(reader.bytes, 5);

    let mut receipt = WorkReceipt::default();
    assert!(StageQueue::load(dir.path(), &budget, Instant::now(), &mut receipt)?.is_none());
    assert_eq!(receipt.status, "budget_exhausted");
    budget.cancellation.cancel();
    assert!(StageQueue::load(dir.path(), &budget, deadline, &mut receipt)?.is_none());
    assert_eq!(receipt.status, "cancelled");
    let mut reader = QueueReader {
        file: File::open(&path)?,
        remaining: 100,
        bytes: 0,
        budget: &budget,
        deadline,
    };
    assert!(reader.read(&mut [0; 1]).is_err());
    assert_eq!(reader.bytes, 0);
    assert_eq!(std::fs::read(path)?, b"123456");
    Ok(())
}
#[test]
fn worker_waits_for_a_reader_then_resumes_within_its_budget() -> Result<()> {
    let (d, db) = setup()?;
    let q = d.path().join("queue");
    let reader = Store::open(&db, true)?;
    let worker_db = db.clone();
    let worker_q = q.clone();
    let worker = std::thread::spawn(move || {
        let mut engine = fake(&worker_db);
        engine.read_probe = true;
        backfill(
            &worker_db,
            StoreOptions::default(),
            &worker_q,
            &mut engine,
            &WorkBudget {
                seconds: 5.,
                ..Default::default()
            },
        )
    });
    let until = Instant::now() + std::time::Duration::from_secs(3);
    while !std::fs::read_to_string(q.join("status.json"))
        .is_ok_and(|s| s.contains("waiting_for_database"))
        && Instant::now() < until
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let waiting = std::fs::read_to_string(q.join("status.json"))?;
    let progress: serde_json::Value = serde_json::from_str(&waiting)?;
    assert_eq!(progress["embedded"], 1);
    assert_eq!(progress["queued_chunks"], 1);
    assert!(
        reader
            .embedding_vectors(
                &fake(&db).profile.id,
                &reader.messages_page(&Filter::default(), None, usize::MAX)?[0]
            )?
            .is_empty()
    );
    drop(reader);
    let result = worker.join().unwrap()?;
    assert_eq!(result.status, "caught_up");
    assert!(result.database_waits > 0);
    assert_eq!(result.merged, 1);
    Ok(())
}
#[test]
fn lock_wait_stops_at_deadline_and_cancellation_preserves_pending_work() -> Result<()> {
    let (d, db) = setup()?;
    let reader = Store::open(&db, true)?;
    let mut engine = fake(&db);
    engine.read_probe = true;
    let started = Instant::now();
    let result = backfill(
        &db,
        StoreOptions::default(),
        &d.path().join("short"),
        &mut engine,
        &WorkBudget {
            seconds: 0.15,
            ..Default::default()
        },
    )?;
    assert_eq!(result.status, "budget_exhausted");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // The deadline may expire during queue preparation on a busy machine;
    // either way the uncommitted document must remain resumable below.
    assert!(engine.calls <= 1);
    let token = llm_history_models::CancellationToken::default();
    let cancel = token.clone();
    let worker_db = db.clone();
    let queue = d.path().join("cancel");
    let worker_q = queue.clone();
    let worker = std::thread::spawn(move || {
        let mut engine = fake(&worker_db);
        engine.read_probe = true;
        backfill(
            &worker_db,
            StoreOptions::default(),
            &worker_q,
            &mut engine,
            &WorkBudget {
                seconds: 5.,
                cancellation: token,
                ..Default::default()
            },
        )
    });
    let until = Instant::now() + std::time::Duration::from_secs(3);
    while !std::fs::read_to_string(queue.join("status.json"))
        .is_ok_and(|s| s.contains("waiting_for_database"))
        && Instant::now() < until
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cancel.cancel();
    assert_eq!(worker.join().unwrap()?.status, "cancelled");
    drop(reader);
    assert_eq!(
        backfill(
            &db,
            StoreOptions::default(),
            &d.path().join("resume"),
            &mut engine,
            &WorkBudget::default()
        )?
        .merged,
        1
    );
    Ok(())
}
#[test]
fn staged_chunks_retain_identity_without_repeating_document_text() -> Result<()> {
    let (d, db) = setup()?;
    let mut engine = fake(&db);
    engine.profile.max_tokens = 8;
    let queue = d.path().join("queue");
    let result = backfill(
        &db,
        StoreOptions::default(),
        &queue,
        &mut engine,
        &WorkBudget {
            tokens: 8,
            ..Default::default()
        },
    )?;
    assert_eq!(result.status, "budget_exhausted");
    assert_eq!(result.queued_chunks, 1);
    let staged = StageQueue::load(
        &queue,
        &WorkBudget::default(),
        Instant::now() + std::time::Duration::from_secs(300),
        &mut WorkReceipt::default(),
    )?
    .unwrap();
    for stage in staged.files.iter().flat_map(|f| &f.entries) {
        assert!(stage.message.text.is_empty());
        assert!(!stage.message.text_hash.is_empty());
    }
    Ok(())
}
#[test]
fn cancellation_during_merge_contention_leaves_a_committable_stage() -> Result<()> {
    struct Gate {
        inner: Fake,
        ready: std::sync::mpsc::SyncSender<()>,
        go: std::sync::mpsc::Receiver<()>,
    }
    impl TokenCounter for Gate {
        fn token_count(&mut self, text: &str) -> Result<usize> {
            self.inner.token_count(text)
        }
    }
    impl Embedder for Gate {
        fn profile(&self) -> &ModelProfile {
            &self.inner.profile
        }
        fn embed(&mut self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>> {
            self.ready.send(())?;
            self.go.recv_timeout(std::time::Duration::from_secs(5))?;
            let _ = query;
            Ok(texts.iter().map(|_| vec![1., 0.]).collect())
        }
    }
    let (d, db) = setup()?;
    let queue = d.path().join("queue");
    let worker_db = db.clone();
    let worker_q = queue.clone();
    let (ready_tx, ready) = std::sync::mpsc::sync_channel(1);
    let (go_tx, go) = std::sync::mpsc::sync_channel(1);
    let token = llm_history_models::CancellationToken::default();
    let cancel = token.clone();
    let worker = std::thread::spawn(move || {
        let mut engine = Gate {
            inner: fake(&worker_db),
            ready: ready_tx,
            go,
        };
        backfill(
            &worker_db,
            StoreOptions::default(),
            &worker_q,
            &mut engine,
            &WorkBudget {
                seconds: 5.,
                cancellation: token,
                ..Default::default()
            },
        )
    });
    ready.recv_timeout(std::time::Duration::from_secs(3))?;
    let reader = Store::open(&db, true)?;
    go_tx.send(())?;
    let until = Instant::now() + std::time::Duration::from_secs(3);
    while !std::fs::read_to_string(queue.join("status.json"))
        .is_ok_and(|s| s.contains("waiting_for_database"))
        && Instant::now() < until
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cancel.cancel();
    let result = worker.join().unwrap()?;
    assert_eq!(result.status, "cancelled");
    assert_eq!(result.queued_chunks, 1);
    drop(reader);
    let mut resumed = fake(&db);
    let result = backfill(
        &db,
        StoreOptions::default(),
        &queue,
        &mut resumed,
        &WorkBudget::default(),
    )?;
    assert_eq!(result.merged, 1);
    assert_eq!(resumed.calls, 0);
    assert_eq!(result.queued_chunks, 0);
    Ok(())
}
#[test]
fn scoped_worker_publishes_only_selected_profiles_including_staged_work() -> Result<()> {
    let (d, db) = setup()?;
    let queue = d.path().join("queue");
    let mut engine = fake(&db);
    let store = Store::open(&db, false)?;
    let root = d.path().join("work");
    std::fs::create_dir_all(root.join("sessions"))?;
    store.add_profile("work", llm_history_types::Harness::Codex, &root, None, None)?;
    std::fs::write(
        root.join("sessions/work.jsonl"),
        llm_history_core::fixture_record(
            llm_history_types::Harness::Codex,
            "thread",
            "work document",
        )
        .iter()
        .map(|r| format!("{r}\n"))
        .collect::<String>(),
    )?;
    crate::fixtures::capture(&store, &[], false)?;
    store.register_model_profile(&engine.profile.id, &serde_json::to_string(&engine.profile)?)?;
    let mut personal = store
        .messages_page(
            &Filter {
                profiles: vec!["personal".into()],
                ..Default::default()
            },
            None,
            usize::MAX,
        )?
        .remove(0);
    personal.text.clear();
    let stage = Staged {
        database: store.identity()?,
        profile: engine.profile.id.clone(),
        message: personal.clone(),
        chunk: 0,
        total: 1,
        vector: Some(vec![1., 0.]),
        failure: None,
    };
    let staged = queue.join(format!("{}-0.json", personal.id));
    atomic_json(&staged, &stage)?;
    drop(store);
    let result = backfill(
        &db,
        StoreOptions::default(),
        &queue,
        &mut engine,
        &WorkBudget {
            profiles: vec!["work".into()],
            ..Default::default()
        },
    )?;
    assert_eq!(result.merged, 1);
    assert!(staged.exists());
    let store = Store::open(&db, true)?;
    assert!(
        store
            .embedding_vectors(&engine.profile.id, &personal)?
            .is_empty()
    );
    drop(store);
    let calls = engine.calls;
    let result = backfill(
        &db,
        StoreOptions::default(),
        &queue,
        &mut engine,
        &WorkBudget {
            profiles: vec!["personal".into()],
            ..Default::default()
        },
    )?;
    assert_eq!(result.merged, 1);
    assert_eq!(engine.calls, calls);
    assert!(!staged.exists());
    Ok(())
}
fn many(count: usize) -> Result<(tempfile::TempDir, std::path::PathBuf)> {
    let d = tempfile::tempdir()?;
    let db = d.path().join("test.duckdb");
    let root = d.path().join("codex");
    std::fs::create_dir_all(root.join("sessions"))?;
    let s = Store::open(&db, false)?;
    s.add_profile(
        "personal",
        llm_history_types::Harness::Codex,
        &root,
        None,
        None,
    )?;
    for i in 0..count {
        let records = llm_history_core::fixture_record(
            llm_history_types::Harness::Codex,
            &format!("thread-{i}"),
            &format!("document {i}"),
        );
        std::fs::write(
            root.join(format!("sessions/{i:02}.jsonl")),
            records.iter().map(|x| format!("{x}\n")).collect::<String>(),
        )?;
    }
    crate::fixtures::capture(&s, &[], false)?;
    Ok((d, db))
}
#[test]
fn documents_share_embedding_requests_and_one_poison_chunk_is_isolated() -> Result<()> {
    struct Batching {
        inner: Fake,
        requests: Vec<usize>,
    }
    impl TokenCounter for Batching {
        fn token_count(&mut self, text: &str) -> Result<usize> {
            self.inner.token_count(text)
        }
    }
    impl Embedder for Batching {
        fn profile(&self) -> &ModelProfile {
            &self.inner.profile
        }
        fn embed(&mut self, texts: &[String], _: bool) -> Result<Vec<Vec<f32>>> {
            self.requests.push(texts.len());
            if texts
                .iter()
                .any(|t| t == "document 3" || t.contains("poison"))
            {
                bail!("poison");
            }
            Ok(texts.iter().map(|_| vec![1., 0.]).collect())
        }
    }
    let (d, db) = many(10)?;
    let mut inner = fake(&db);
    inner.profile.max_tokens = 1000;
    let mut engine = Batching {
        inner,
        requests: vec![],
    };
    let receipt = backfill(
        &db,
        StoreOptions::default(),
        &d.path().join("queue"),
        &mut engine,
        &WorkBudget::default(),
    )?;
    assert_eq!(receipt.status, "caught_up");
    assert_eq!(receipt.merged, 9);
    assert_eq!(receipt.failures, 1);
    assert_eq!(receipt.embedded, 9);
    // One batched request fails, then each chunk is retried alone.
    assert_eq!(engine.requests[0], 10);
    assert_eq!(engine.requests.len(), 11);
    let s = Store::open(&db, true)?;
    let failures = s.sql_bounded(
        "select count(*) as n from embedding_failures",
        10,
        1000,
        100_000,
        false,
        SQL_TIMEOUT,
    )?;
    assert!(failures.to_string().contains('1'));

    // A successful leading chunk must not reset failures of whole documents.
    let (d, db) = many(5)?;
    for i in 0..5 {
        let records = llm_history_core::fixture_record(
            llm_history_types::Harness::Codex,
            &format!("thread-{i}"),
            &format!("{}poison {i}", "a".repeat(40)),
        );
        std::fs::write(
            d.path().join(format!("codex/sessions/{i:02}.jsonl")),
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )?;
    }
    crate::fixtures::capture(&Store::open(&db, false)?, &[], false)?;
    let mut inner = fake(&db);
    inner.profile.max_tokens = 32;
    let mut engine = Batching {
        inner,
        requests: vec![],
    };
    let receipt = backfill(
        &db,
        StoreOptions::default(),
        &d.path().join("queue"),
        &mut engine,
        &WorkBudget::default(),
    )?;
    assert_eq!(receipt.status, "repeated_failures");
    assert_eq!(receipt.failures, 3);
    assert_eq!(receipt.embedded, 0);
    Ok(())
}
#[test]
fn grouped_staging_is_immutable_and_amortizes_file_operations() -> Result<()> {
    let (d, db) = setup()?;
    let directory = d.path().join("batches");
    std::fs::create_dir_all(&directory)?;
    let store = Store::open(&db, true)?;
    let mut message = store
        .messages_page(&Filter::default(), None, usize::MAX)?
        .remove(0);
    message.text.clear();
    let identity = store.identity()?;
    drop(store);
    let mut queue = StageQueue::load(
        &directory,
        &WorkBudget::default(),
        Instant::now() + std::time::Duration::from_secs(300),
        &mut WorkReceipt::default(),
    )?
    .unwrap();
    let mut receipt = WorkReceipt::default();
    let started = Instant::now();
    for batch in 0..8 {
        let entries = (0..32)
            .map(|i| {
                let mut m = message.clone();
                m.id = (batch * 32 + i).to_string();
                Staged {
                    database: identity.clone(),
                    profile: "model".into(),
                    message: m,
                    chunk: 0,
                    total: 1,
                    vector: Some(
                        (0..768)
                            .map(|j| ((batch * 32 + i + j) as f32).sin() * 0.04)
                            .collect(),
                    ),
                    failure: None,
                }
            })
            .collect();
        queue.push(entries, &mut receipt)?;
    }
    assert_eq!(queue.files.len(), 8);
    assert_eq!(receipt.queued_chunks, 256);
    let elapsed = started.elapsed();
    let file = &queue.files[0];
    let path = file.path.clone();
    let before = std::fs::read(&path)?;
    let key = file.entries[0].key();
    queue.acknowledge(&HashSet::from([key]), &mut receipt)?;
    assert_eq!(before, std::fs::read(&path)?);
    assert_eq!(receipt.queued_chunks, 255);
    println!(
        "Rust staging: 256 x 768-dimensional chunks, 8 immutable files, 16 sync calls, {} bytes, {:.1} ms",
        receipt.queued_bytes,
        elapsed.as_secs_f64() * 1000.
    );
    // Reload also sees an unfinished document's previous chunks.
    let mut partial = message.clone();
    partial.id = "partial".into();
    queue.push(
        vec![Staged {
            database: identity.clone(),
            profile: "model".into(),
            message: partial.clone(),
            chunk: 0,
            total: 2,
            vector: Some(vec![1., 0.]),
            failure: None,
        }],
        &mut receipt,
    )?;
    let mut reloaded = StageQueue::load(
        &directory,
        &WorkBudget::default(),
        Instant::now() + std::time::Duration::from_secs(300),
        &mut WorkReceipt::default(),
    )?
    .unwrap();
    assert!(
        reloaded
            .chunks_for(&identity, "model", &partial, 2)
            .contains(&0)
    );
    reloaded.discard_incompatible(&identity, "model", &partial, 3, &mut receipt)?;
    assert!(
        !reloaded
            .chunks_for(&identity, "model", &partial, 2)
            .contains(&0)
    );
    Ok(())
}
#[test]
fn publications_are_bounded_and_replay_does_not_rewrite_committed_vectors() -> Result<()> {
    let (d, db) = many(70)?;
    let directory = d.path().join("staging");
    std::fs::create_dir_all(&directory)?;
    let mut queue = StageQueue::load(
        &directory,
        &WorkBudget::default(),
        Instant::now() + std::time::Duration::from_secs(300),
        &mut WorkReceipt::default(),
    )?
    .unwrap();
    let engine = fake(&db);
    let store = Store::open(&db, true)?;
    let database = store.identity()?;
    let entries = store
        .messages_page(&Filter::default(), None, usize::MAX)?
        .into_iter()
        .map(|mut m| {
            m.text.clear();
            Staged {
                database: database.clone(),
                profile: engine.profile.id.clone(),
                message: m,
                chunk: 0,
                total: 1,
                vector: Some(vec![1., 0.]),
                failure: None,
            }
        })
        .collect();
    drop(store);
    let mut receipt = WorkReceipt::default();
    queue.push(entries, &mut receipt)?;
    let path = queue.files[0].path.clone();
    let before = std::fs::read(&path)?;
    let reader = Store::open(&db, true)?;
    let review = queue.review(
        &reader,
        &database,
        &engine.profile.id,
        &WorkBudget::default(),
    )?;
    assert_eq!(review.documents.len(), MERGE_DOCUMENTS);
    assert_eq!(review.ready_ids.len(), 70);
    drop(reader);
    let prepared = EmbeddingBatch::new(&review.documents)?;
    let publication_started = Instant::now();
    let writer = Store::open(&db, false)?;
    assert_eq!(
        writer.publish_embedding_batch(
            &engine.profile.id,
            &serde_json::to_string(&engine.profile)?,
            &prepared
        )?,
        MERGE_DOCUMENTS
    );
    drop(writer);
    println!(
        "Rust bulk publication: {} documents in {:.1} ms including open/close",
        MERGE_DOCUMENTS,
        publication_started.elapsed().as_secs_f64() * 1000.
    );
    queue.acknowledge(&review.selected, &mut receipt)?;
    assert_eq!(before, std::fs::read(&path)?);
    // The disk file still contains the acknowledged prefix after a process exit.
    let reloaded = StageQueue::load(
        &directory,
        &WorkBudget::default(),
        Instant::now() + std::time::Duration::from_secs(300),
        &mut WorkReceipt::default(),
    )?
    .unwrap();
    let reader = Store::open(&db, true)?;
    let replay = reloaded.review(
        &reader,
        &database,
        &engine.profile.id,
        &WorkBudget::default(),
    )?;
    assert_eq!(replay.acknowledged.len(), MERGE_DOCUMENTS);
    assert_eq!(replay.documents.len(), 70 - MERGE_DOCUMENTS);
    drop(reader);
    let mut engine = fake(&db);
    let result = backfill(
        &db,
        StoreOptions::default(),
        &directory,
        &mut engine,
        &WorkBudget::default(),
    )?;
    assert_eq!(result.status, "caught_up");
    assert_eq!(result.merged, 70 - MERGE_DOCUMENTS);
    assert_eq!(result.queued_chunks, 0);
    assert_eq!(engine.calls, 0);
    assert!(!path.exists());
    Ok(())
}
#[test]
fn semantic_ranking_runs_in_duckdb_over_native_vectors() -> Result<()> {
    struct Query(ModelProfile);
    impl TokenCounter for Query {
        fn token_count(&mut self, text: &str) -> Result<usize> {
            Ok(text.chars().count())
        }
    }
    impl Embedder for Query {
        fn profile(&self) -> &ModelProfile {
            &self.0
        }
        fn embed(&mut self, texts: &[String], _: bool) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1., 0.]).collect())
        }
    }
    let (_d, db) = many(3)?;
    let mut engine = Query(fake(&db).profile);
    let s = Store::open(&db, false)?;
    s.register_model_profile(&engine.0.id, &serde_json::to_string(&engine.0)?)?;
    let messages = s.messages_page(&Filter::default(), None, usize::MAX)?;
    assert_eq!(messages.len(), 3);
    publish_fixture_vectors(
        &s,
        &engine.0,
        &messages,
        &[&[0., 1.], &[1., 0.], &[0.6, 0.8]],
    )?;
    assert_eq!(
        s.embedding_vectors(&engine.0.id, &messages[1])?,
        vec![vec![1., 0.]]
    );
    let r = SearchRequest {
        explain: true,
        query: "anything".into(),
        mode: Mode::Semantic,
        limit: 3,
        per_session: 3,
        ..SearchRequest::default()
    };
    let out = search(|| Ok(&s), &r, Some(&mut engine), None)?;
    let order: Vec<_> = out.results.iter().map(|h| h.message.id.clone()).collect();
    assert_eq!(
        order,
        vec![
            messages[1].id.clone(),
            messages[2].id.clone(),
            messages[0].id.clone()
        ]
    );
    assert!((out.results[0].semantic_score - 1.).abs() < 1e-6);
    assert_eq!(out.details["embedded_documents"], 3);

    // An excluded vector with a different dimension must never be scored.
    let (_scoped_dir, scoped_db) = many(3)?;
    let s = Store::open(&scoped_db, false)?;
    let messages = s.messages_page(&Filter::default(), None, usize::MAX)?;
    publish_fixture_vectors(
        &s,
        &engine.0,
        &messages,
        &[&[1., 0., 0.], &[1., 0.], &[0.6, 0.8]],
    )?;
    let scoped = SearchRequest {
        filter: Filter {
            threads: vec![messages[1].thread_id.clone()],
            ..Default::default()
        },
        ..r
    };
    let out = search(|| Ok(&s), &scoped, Some(&mut engine), None)?;
    assert_eq!(out.results.len(), 1);
    assert_eq!(out.results[0].message.id, messages[1].id);
    Ok(())
}

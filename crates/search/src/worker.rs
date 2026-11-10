//! Bounded background indexing, durable staging, and cooperative publication.
use crate::eligible;
use anyhow::{Context, Result, bail};
use llm_history_models::{Embedder, InferenceError, split_for_tokens};
use llm_history_store::{EmbeddingBatch, EmbeddingDocument, Store};
use llm_history_types::jobs::WorkReceipt;
use llm_history_types::{query::Message, storage::StoreOptions};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
#[derive(Debug, Clone)]
pub struct WorkBudget {
    pub seconds: f64,
    pub documents: usize,
    pub tokens: usize,
    pub queue_bytes: u64,
    pub queue_items: usize,
    pub max_document_chars: usize,
    pub read_batch: usize,
    pub retry_failures: bool,
    pub profiles: Vec<String>,
    pub source: Option<String>,
    pub cancellation: llm_history_models::CancellationToken,
}
impl Default for WorkBudget {
    fn default() -> Self {
        Self {
            seconds: 300.,
            documents: 10000,
            tokens: 2_000_000,
            queue_bytes: 64 * 1024 * 1024,
            queue_items: 1024,
            max_document_chars: 1_000_000,
            read_batch: 64,
            retry_failures: false,
            profiles: vec![],
            source: None,
            cancellation: Default::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Staged {
    database: String,
    profile: String,
    message: Message,
    chunk: usize,
    total: usize,
    vector: Option<Vec<f32>>,
    failure: Option<String>,
}
type StageKey = (String, String, String, String);
impl Staged {
    fn key(&self) -> StageKey {
        (
            self.database.clone(),
            self.profile.clone(),
            self.message.id.clone(),
            self.message.text_hash.clone(),
        )
    }
}
// Check item limits before decoding each entry, including entries within one batch.
struct StageEntries<'a> {
    remaining: &'a mut usize,
    budget: &'a WorkBudget,
    deadline: Instant,
}
impl StageEntries<'_> {
    fn check<E: serde::de::Error>(&self) -> std::result::Result<(), E> {
        if self.budget.cancellation.is_cancelled() {
            return Err(E::custom("worker cancelled while loading staging"));
        }
        if Instant::now() >= self.deadline {
            return Err(E::custom("worker deadline reached while loading staging"));
        }
        if *self.remaining == 0 {
            return Err(E::custom(
                "durable staging queue exceeds queue_items; staged work retained; increase the worker queue budget",
            ));
        }
        Ok(())
    }
}
impl<'de> DeserializeSeed<'de> for StageEntries<'_> {
    type Value = Vec<Staged>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        de: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        de.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for StageEntries<'_> {
    type Value = Vec<Staged>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a staged entry or array of staged entries")
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = seq.next_element_seed(StageEntry {
            remaining: self.remaining,
            budget: self.budget,
            deadline: self.deadline,
        })? {
            entries.push(entry);
        }
        Ok(entries)
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> std::result::Result<Self::Value, A::Error> {
        self.check()?;
        let entry = Staged::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        *self.remaining -= 1;
        Ok(vec![entry])
    }
}
struct StageEntry<'a> {
    remaining: &'a mut usize,
    budget: &'a WorkBudget,
    deadline: Instant,
}
impl<'de> DeserializeSeed<'de> for StageEntry<'_> {
    type Value = Staged;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        de: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        StageEntries {
            remaining: self.remaining,
            budget: self.budget,
            deadline: self.deadline,
        }
        .check()?;
        let entry = Staged::deserialize(de)?;
        *self.remaining -= 1;
        Ok(entry)
    }
}
struct QueueReader<'a> {
    file: File,
    remaining: u64,
    bytes: u64,
    budget: &'a WorkBudget,
    deadline: Instant,
}
impl Read for QueueReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.budget.cancellation.is_cancelled() || Instant::now() >= self.deadline {
            return Err(std::io::Error::other("staging acquisition interrupted"));
        }
        if self.remaining == 0 {
            return match self.file.read(&mut [0])? {
                0 => Ok(0),
                _ => Err(std::io::Error::other(
                    "durable staging queue exceeds queue_bytes; staged work retained; increase the worker queue budget",
                )),
            };
        }
        let limit = (buf.len() as u64).min(self.remaining) as usize;
        let n = self.file.read(&mut buf[..limit])?;
        self.remaining -= n as u64;
        self.bytes += n as u64;
        Ok(n)
    }
}
struct StageFile {
    path: PathBuf,
    entries: Vec<Staged>,
    bytes: u64,
}
struct StageQueue {
    directory: PathBuf,
    files: Vec<StageFile>,
}
struct QueueReview {
    acknowledged: HashSet<StageKey>,
    selected: HashSet<StageKey>,
    documents: Vec<EmbeddingDocument>,
    ready_ids: Vec<i64>,
}
// Bound publication by documents and vector data. One larger document is published alone.
const MERGE_DOCUMENTS: usize = 64;
const MERGE_CHUNKS: usize = 128;
impl StageQueue {
    fn load(
        directory: &Path,
        budget: &WorkBudget,
        deadline: Instant,
        receipt: &mut WorkReceipt,
    ) -> Result<Option<Self>> {
        let mut files = Vec::new();
        let mut bytes = 0u64;
        let mut remaining_items = budget.queue_items;
        for entry in std::fs::read_dir(directory)? {
            if stopped(budget, deadline, receipt) {
                return Ok(None);
            }
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json")
                || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| matches!(n, "status.json" | "launch.json"))
            {
                continue;
            }
            let file = File::open(&path)?;
            if file.metadata()?.len() > budget.queue_bytes.saturating_sub(bytes) {
                bail!(
                    "durable staging queue exceeds queue_bytes ({}); staged work retained; increase the worker queue budget",
                    budget.queue_bytes
                );
            }
            let mut reader = QueueReader {
                file,
                remaining: budget.queue_bytes - bytes,
                bytes: 0,
                budget,
                deadline,
            };
            let mut de = serde_json::Deserializer::from_reader(BufReader::new(&mut reader));
            let decoded = StageEntries {
                remaining: &mut remaining_items,
                budget,
                deadline,
            }
            .deserialize(&mut de)
            .and_then(|entries| {
                de.end()?;
                Ok(entries)
            });
            let entries = match decoded {
                Ok(entries) => entries,
                Err(_) if stopped(budget, deadline, receipt) => return Ok(None),
                Err(error) => {
                    return Err(error)
                        .context("could not load durable staging queue; staged work retained");
                }
            };
            bytes += reader.bytes;
            // Empty files still consume the byte budget, but need no resident entries.
            if !entries.is_empty() {
                files.push(StageFile {
                    path,
                    entries,
                    bytes: reader.bytes,
                });
            }
        }
        if stopped(budget, deadline, receipt) {
            return Ok(None);
        }
        Ok(Some(Self {
            directory: directory.to_owned(),
            files,
        }))
    }
    fn update_status(&self, receipt: &mut WorkReceipt) {
        receipt.queued_chunks = self.files.iter().map(|f| f.entries.len()).sum();
        receipt.queued_bytes = self.files.iter().map(|f| f.bytes).sum();
    }
    fn chunks_for(
        &self,
        database: &str,
        model: &str,
        message: &Message,
        total: usize,
    ) -> HashSet<usize> {
        self.files
            .iter()
            .flat_map(|f| &f.entries)
            .filter(|s| {
                s.database == database
                    && s.profile == model
                    && s.message.id == message.id
                    && s.message.text_hash == message.text_hash
                    && s.total == total
                    && s.vector.is_some()
            })
            .map(|s| s.chunk)
            .collect()
    }
    fn discard_incompatible(
        &mut self,
        database: &str,
        model: &str,
        message: &Message,
        total: usize,
        receipt: &mut WorkReceipt,
    ) -> Result<()> {
        let keys = self
            .files
            .iter()
            .flat_map(|f| &f.entries)
            .filter(|s| {
                s.database == database
                    && s.profile == model
                    && s.message.id == message.id
                    && s.message.text_hash == message.text_hash
                    && s.total != total
            })
            .map(Staged::key)
            .collect();
        self.acknowledge(&keys, receipt)
    }
    fn push(&mut self, stages: Vec<Staged>, receipt: &mut WorkReceipt) -> Result<()> {
        // Keep unfinished documents separate from completed documents. This lets us
        // acknowledge whole immutable files without rewriting vectors after each merge.
        let complete: HashSet<_> = stages
            .iter()
            .filter(|s| s.failure.is_some() || s.chunk + 1 == s.total)
            .map(|s| s.message.id.clone())
            .collect();
        let mut groups = HashMap::<_, Vec<_>>::new();
        for stage in stages {
            let unfinished =
                (!complete.contains(&stage.message.id)).then(|| stage.message.id.clone());
            groups
                .entry((stage.message.profile.clone(), unfinished))
                .or_default()
                .push(stage);
        }
        for entries in groups.into_values() {
            let data = serde_json::to_vec(&entries)?;
            let mut temp = tempfile::Builder::new()
                .prefix(".stage-")
                .tempfile_in(&self.directory)?;
            let path = temp.path().with_extension("json");
            temp.write_all(&data)?;
            temp.as_file().sync_all()?;
            temp.persist_noclobber(&path).map_err(|e| e.error)?;
            #[cfg(unix)]
            File::open(&self.directory)?.sync_all()?;
            self.files.push(StageFile {
                path,
                entries,
                bytes: data.len() as u64,
            });
        }
        self.update_status(receipt);
        Ok(())
    }
    fn acknowledge(&mut self, keys: &HashSet<StageKey>, receipt: &mut WorkReceipt) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        for file in &mut self.files {
            file.entries.retain(|s| !keys.contains(&s.key()));
            if file.entries.is_empty() {
                std::fs::remove_file(&file.path)?;
            }
        }
        self.files.retain(|f| !f.entries.is_empty());
        self.update_status(receipt);
        Ok(())
    }
    fn review(
        &self,
        store: &Store,
        database: &str,
        model: &str,
        budget: &WorkBudget,
    ) -> Result<QueueReview> {
        let mut groups = BTreeMap::<_, Vec<_>>::new();
        for stage in self.files.iter().flat_map(|f| &f.entries) {
            if stage.profile == model {
                groups.entry(stage.key()).or_default().push(stage);
            }
        }
        let ids = groups
            .keys()
            .filter(|k| k.0 == database)
            .map(|k| k.2.parse())
            .collect::<std::result::Result<Vec<i64>, _>>()?;
        let states =
            store.embedding_states(model, &ids, &budget.profiles, budget.source.as_deref())?;
        let mut out = QueueReview {
            acknowledged: HashSet::new(),
            selected: HashSet::new(),
            documents: vec![],
            ready_ids: vec![],
        };
        let mut chunks = 0;
        for (key, mut group) in groups {
            let id: i64 = key.2.parse()?;
            let state = states.get(&id);
            if key.0 != database || state.is_none_or(|s| s.text_hash != key.3 || s.published) {
                out.acknowledged.insert(key);
                continue;
            }
            if !state.unwrap().in_scope {
                continue;
            }
            let failure = group.iter().find_map(|s| s.failure.clone());
            group.sort_by_key(|s| s.chunk);
            group.dedup_by_key(|s| s.chunk);
            let first = group[0];
            if failure.is_none()
                && (group.len() != first.total
                    || group
                        .iter()
                        .enumerate()
                        .any(|(i, s)| s.chunk != i || s.total != first.total || s.vector.is_none()))
            {
                continue;
            }
            out.ready_ids.push(id);
            if out.documents.len() >= MERGE_DOCUMENTS
                || (!out.documents.is_empty() && chunks + group.len() > MERGE_CHUNKS)
            {
                continue;
            }
            let vectors = if failure.is_some() {
                vec![]
            } else {
                group.iter().map(|s| s.vector.clone().unwrap()).collect()
            };
            chunks += group.len();
            out.documents.push(EmbeddingDocument {
                id,
                text_hash: key.3.clone(),
                vectors,
                failure,
            });
            out.selected.insert(key);
        }
        Ok(out)
    }
}
fn stopped(b: &WorkBudget, deadline: Instant, r: &mut WorkReceipt) -> bool {
    if b.cancellation.is_cancelled() {
        r.status = "cancelled".into();
        true
    } else if Instant::now() >= deadline {
        r.status = "budget_exhausted".into();
        true
    } else {
        false
    }
}
fn worker_store(
    db: &Path,
    storage: StoreOptions,
    queue: &Path,
    b: &WorkBudget,
    deadline: Instant,
    r: &mut WorkReceipt,
    read_only: bool,
) -> Result<Option<Store>> {
    let mut delay = std::time::Duration::from_millis(50);
    loop {
        if stopped(b, deadline, r) {
            return Ok(None);
        }
        match Store::open_with_options(db, read_only, storage) {
            Ok(store) => {
                r.status = "running".into();
                return Ok(Some(store));
            }
            Err(e)
                if e.downcast_ref::<llm_history_types::storage::DatabaseBusy>()
                    .is_some() =>
            {
                r.status = "waiting_for_database".into();
                r.elapsed_seconds = (b.seconds
                    - deadline
                        .saturating_duration_since(Instant::now())
                        .as_secs_f64())
                .max(0.);
                r.database_waits += 1;
                write_status(queue, r, false)?;
                let until = (Instant::now() + delay).min(deadline);
                while Instant::now() < until && !b.cancellation.is_cancelled() {
                    std::thread::sleep(
                        until
                            .saturating_duration_since(Instant::now())
                            .min(std::time::Duration::from_millis(50)),
                    );
                }
                delay = (delay * 2).min(std::time::Duration::from_secs(1));
            }
            Err(e) => return Err(e),
        }
    }
}
fn transient(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<InferenceError>()
        .is_some_and(|e| match e {
            InferenceError::Http(s) => *s >= 500 || *s == 429,
            InferenceError::Transport => true,
            _ => false,
        })
}
/// Credential and endpoint errors affect every document, so they end the run
/// without recording per-document failures.
fn rejected(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<InferenceError>()
        .is_some_and(|e| matches!(e, InferenceError::Http(401 | 403 | 404 | 405 | 407)))
}
/// Chunks per embedding request; the request also stays within the model's token window.
const EMBED_BATCH: usize = 32;
struct Pending {
    stage: Staged,
    text: String,
    tokens: usize,
}
/// Embeds the batch in one request. Returns true when the run must stop.
#[allow(clippy::too_many_arguments)]
fn flush(
    engine: &mut dyn Embedder,
    queue: &mut StageQueue,
    batch: &mut Vec<Pending>,
    b: &WorkBudget,
    deadline: Instant,
    r: &mut WorkReceipt,
    consecutive: &mut usize,
    failed: &mut HashSet<String>,
) -> Result<bool> {
    let items = std::mem::take(batch);
    if items.is_empty() {
        return Ok(false);
    }
    let texts: Vec<String> = items.iter().map(|p| p.text.clone()).collect();
    match engine.embed(&texts, false) {
        Ok(vectors) => {
            if vectors.len() != items.len() {
                bail!("embedder returned wrong number of vectors");
            }
            let dimensions = engine.profile().dimensions;
            let mut stages = Vec::with_capacity(items.len());
            for (mut item, vector) in items.into_iter().zip(vectors) {
                item.stage.vector = Some(llm_history_models::normalize(&vector, dimensions)?);
                r.tokens += item.tokens;
                if item.stage.chunk + 1 == item.stage.total
                    && !failed.contains(&item.stage.message.id)
                {
                    r.embedded += 1;
                    *consecutive = 0;
                }
                stages.push(item.stage);
            }
            queue.push(stages, r)?;
        }
        Err(error) => {
            if stopped(b, deadline, r) {
                return Ok(true);
            }
            if transient(&error) {
                r.status = "transient_failure".into();
                return Ok(true);
            }
            if rejected(&error) {
                return Err(error.context("embedding endpoint rejected the request"));
            }
            if items.len() > 1 {
                // Retry one at a time so a single bad chunk does not fail its neighbours.
                for item in items {
                    if failed.contains(&item.stage.message.id) {
                        continue;
                    }
                    if flush(
                        engine,
                        queue,
                        &mut vec![item],
                        b,
                        deadline,
                        r,
                        consecutive,
                        failed,
                    )? {
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
            let mut item = items.into_iter().next().context("empty batch")?;
            item.stage.failure = Some("embedding_error".into());
            if failed.insert(item.stage.message.id.clone()) {
                r.failures += 1;
                *consecutive += 1;
            }
            queue.push(vec![item.stage], r)?;
            if *consecutive >= 3 {
                r.status = "repeated_failures".into();
                return Ok(true);
            }
        }
    }
    r.elapsed_seconds = (b.seconds
        - deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64())
    .max(0.);
    write_status(&queue.directory, r, false)?;
    Ok(false)
}
pub fn backfill(
    db: &Path,
    storage: StoreOptions,
    queue: &Path,
    engine: &mut dyn Embedder,
    budget: &WorkBudget,
) -> Result<WorkReceipt> {
    if !budget.seconds.is_finite()
        || budget.seconds <= 0.
        || budget.documents == 0
        || budget.tokens == 0
        || budget.queue_bytes == 0
        || budget.queue_items == 0
        || budget.max_document_chars == 0
        || !(1..=256).contains(&budget.read_batch)
    {
        bail!("worker budgets must be positive; read batch must be 1..=256");
    }
    std::fs::create_dir_all(queue)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(queue.join("worker.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("embedding worker already running")?;
    let started = Instant::now();
    let deadline = started
        .checked_add(std::time::Duration::try_from_secs_f64(budget.seconds)?)
        .context("worker deadline out of range")?;
    engine.set_deadline(deadline);
    let profile = engine.profile().clone();
    let mut receipt = WorkReceipt {
        status: "running".into(),
        pid: std::process::id(),
        model_profile: profile.id.clone(),
        profiles: budget.profiles.clone(),
        ..Default::default()
    };
    let contract = serde_json::to_string(&profile)?;
    let Some(mut staging) = StageQueue::load(queue, budget, deadline, &mut receipt)? else {
        receipt.elapsed_seconds = started.elapsed().as_secs_f64();
        write_status(queue, &mut receipt, true)?;
        return Ok(receipt);
    };
    staging.update_status(&mut receipt);
    let mut identity = None;
    if budget.retry_failures
        && let Some(store) =
            worker_store(db, storage, queue, budget, deadline, &mut receipt, false)?
    {
        identity = Some(store.identity()?);
        store.retry_failures(&profile.id, &budget.profiles, budget.source.as_deref())?;
    }
    let mut after = i64::MIN;
    let mut consecutive = 0;
    let mut failed = HashSet::new();
    let mut batch: Vec<Pending> = Vec::new();
    'pages: loop {
        let Some(store) = worker_store(db, storage, queue, budget, deadline, &mut receipt, true)?
        else {
            break;
        };
        let current = store.identity()?;
        if identity.as_ref().is_some_and(|old| old != &current) {
            receipt.status = "database_replaced".into();
            break;
        }
        identity.get_or_insert_with(|| current.clone());
        let review = staging.review(&store, &current, &profile.id, budget)?;
        let at_limit = receipt.attempted >= budget.documents || receipt.tokens >= budget.tokens;
        let page = if at_limit {
            llm_history_store::PendingPage {
                messages: vec![],
                last_id: after,
                oversized: 0,
            }
        } else {
            store.pending_messages(
                &profile.id,
                &budget.profiles,
                budget.source.as_deref(),
                after,
                budget.read_batch.min(budget.documents - receipt.attempted),
                budget.max_document_chars,
                &review.ready_ids,
            )?
        };
        drop(store);
        // Parsing, grouping, binding construction and acknowledgement all happen
        // outside the main database's exclusive writer window.
        staging.acknowledge(&review.acknowledged, &mut receipt)?;
        if !review.documents.is_empty() {
            let prepared = EmbeddingBatch::new(&review.documents)?;
            let writer = if page.messages.is_empty() && !at_limit {
                worker_store(db, storage, queue, budget, deadline, &mut receipt, false)?
            } else {
                match Store::open_with_options(db, false, storage) {
                    Ok(store) => Some(store),
                    Err(e)
                        if e.downcast_ref::<llm_history_types::storage::DatabaseBusy>()
                            .is_some() =>
                    {
                        None
                    }
                    Err(e) => return Err(e),
                }
            };
            if let Some(writer) = writer {
                if writer.identity()? != current {
                    receipt.status = "database_replaced".into();
                    break;
                }
                receipt.merged +=
                    writer.publish_embedding_batch(&profile.id, &contract, &prepared)?;
                drop(writer);
                staging.acknowledge(&review.selected, &mut receipt)?;
            } else if page.messages.is_empty() && !at_limit {
                break;
            }
        }
        if at_limit {
            receipt.status = "budget_exhausted".into();
            break;
        }
        receipt.oversized = page.oversized;
        if page.messages.is_empty() {
            // Drain bounded publications before declaring the queue caught up.
            if !review.ready_ids.is_empty() {
                continue;
            }
            receipt.status = if page.oversized > 0 {
                "input_limit"
            } else {
                "caught_up"
            }
            .into();
            break;
        }
        after = page.last_id;
        let (mut queued_items, mut queued_bytes) = (receipt.queued_chunks, receipt.queued_bytes);
        let mut batch_tokens = 0;
        for message in page.messages.into_iter().filter(eligible) {
            if stopped(budget, deadline, &mut receipt) {
                break 'pages;
            }
            receipt.attempted += 1;
            let chunks = match split_for_tokens(
                engine,
                &message.text,
                &profile.document_prefix,
                profile.max_tokens,
            ) {
                Ok(chunks) => chunks,
                Err(error) => {
                    if stopped(budget, deadline, &mut receipt) {
                        break 'pages;
                    }
                    if transient(&error) {
                        receipt.status = "transient_failure".into();
                        break 'pages;
                    }
                    if rejected(&error) {
                        return Err(error.context("tokenizer endpoint rejected the request"));
                    }
                    // Finish earlier documents before counting this document's failure.
                    if flush(
                        engine,
                        &mut staging,
                        &mut batch,
                        budget,
                        deadline,
                        &mut receipt,
                        &mut consecutive,
                        &mut failed,
                    )? {
                        break 'pages;
                    }
                    batch_tokens = 0;
                    let mut metadata = message.clone();
                    metadata.text.clear();
                    staging.push(
                        vec![Staged {
                            database: current.clone(),
                            profile: profile.id.clone(),
                            message: metadata,
                            chunk: 0,
                            total: 1,
                            vector: None,
                            failure: Some("tokenization_error".into()),
                        }],
                        &mut receipt,
                    )?;
                    failed.insert(message.id.clone());
                    receipt.failures += 1;
                    consecutive += 1;
                    if consecutive >= 3 {
                        receipt.status = "repeated_failures".into();
                        break 'pages;
                    }
                    continue;
                }
            };
            let total = chunks.len();
            staging.discard_incompatible(&current, &profile.id, &message, total, &mut receipt)?;
            let present = staging.chunks_for(&current, &profile.id, &message, total);
            let mut queued = false;
            for (chunk, (text, tokens)) in chunks.into_iter().enumerate() {
                if present.contains(&chunk) {
                    continue;
                }
                if !batch.is_empty()
                    && (batch.len() >= EMBED_BATCH || batch_tokens + tokens > profile.max_tokens)
                {
                    batch_tokens = 0;
                    if flush(
                        engine,
                        &mut staging,
                        &mut batch,
                        budget,
                        deadline,
                        &mut receipt,
                        &mut consecutive,
                        &mut failed,
                    )? {
                        break 'pages;
                    }
                }
                let mut metadata = message.clone();
                metadata.text.clear();
                let stage = Staged {
                    database: current.clone(),
                    profile: profile.id.clone(),
                    message: metadata,
                    chunk,
                    total,
                    vector: None,
                    failure: None,
                };
                let estimate = serde_json::to_vec(&stage)?.len() as u64
                    + (profile.dimensions as u64) * 24
                    + 1024;
                if receipt
                    .tokens
                    .saturating_add(batch_tokens)
                    .saturating_add(tokens)
                    > budget.tokens
                    || queued_items >= budget.queue_items
                    || queued_bytes.saturating_add(estimate) > budget.queue_bytes
                {
                    if !flush(
                        engine,
                        &mut staging,
                        &mut batch,
                        budget,
                        deadline,
                        &mut receipt,
                        &mut consecutive,
                        &mut failed,
                    )? {
                        receipt.status = "budget_exhausted".into();
                    }
                    break 'pages;
                }
                queued_items += 1;
                queued_bytes += estimate;
                batch_tokens += tokens;
                batch.push(Pending {
                    stage,
                    text,
                    tokens,
                });
                queued = true;
            }
            if !queued && !failed.contains(&message.id) {
                // Every chunk was already staged by an earlier run.
                receipt.embedded += 1;
                consecutive = 0;
            }
        }
        if flush(
            engine,
            &mut staging,
            &mut batch,
            budget,
            deadline,
            &mut receipt,
            &mut consecutive,
            &mut failed,
        )? {
            break 'pages;
        }
    }
    // One final nonblocking, bounded publication; contention leaves stages for resume.
    if receipt.status != "database_replaced" {
        let reader = match Store::open_with_options(db, true, storage) {
            Ok(store) => Some(store),
            Err(e)
                if e.downcast_ref::<llm_history_types::storage::DatabaseBusy>()
                    .is_some() =>
            {
                None
            }
            Err(e) => return Err(e),
        };
        if let Some(reader) = reader {
            let current = reader.identity()?;
            if identity.as_ref().is_some_and(|id| id == &current) {
                let review = staging.review(&reader, &current, &profile.id, budget)?;
                drop(reader);
                staging.acknowledge(&review.acknowledged, &mut receipt)?;
                if !review.documents.is_empty() {
                    let prepared = EmbeddingBatch::new(&review.documents)?;
                    match Store::open_with_options(db, false, storage) {
                        Ok(writer) if writer.identity()? == current => {
                            receipt.merged += writer.publish_embedding_batch(
                                &profile.id,
                                &contract,
                                &prepared,
                            )?;
                            drop(writer);
                            staging.acknowledge(&review.selected, &mut receipt)?;
                        }
                        Ok(_) => {
                            receipt.status = "database_replaced".into();
                        }
                        Err(e)
                            if e.downcast_ref::<llm_history_types::storage::DatabaseBusy>()
                                .is_some() => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
    receipt.elapsed_seconds = started.elapsed().as_secs_f64();
    receipt.next_launch_at = chrono::Utc::now().timestamp()
        + match receipt.status.as_str() {
            "caught_up" => 0,
            "failed" | "transient_failure" | "repeated_failures" | "input_limit" => 1800,
            _ => 30,
        };
    write_status(queue, &mut receipt, true)?;
    Ok(receipt)
}
use std::fs::File;

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    publish_json(path, value, true)
}
/// Progress is replaceable; only staged inference results require durable writes.
pub fn status_json(path: &Path, value: &impl Serialize) -> Result<()> {
    publish_json(path, value, false)
}
fn publish_json(path: &Path, value: &impl Serialize, durable: bool) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&serde_json::to_vec(value)?)?;
    if durable {
        tmp.as_file().sync_all()?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    if durable {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
fn write_status(queue: &Path, receipt: &mut WorkReceipt, force: bool) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    if !force && now - receipt.updated_at < 2 {
        return Ok(());
    }
    receipt.updated_at = now;
    status_json(&queue.join("status.json"), receipt)
}

#[cfg(test)]
mod tests;

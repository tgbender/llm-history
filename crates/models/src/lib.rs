//! Engine-independent inference contracts and a managed llama.cpp implementation.
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
mod cache;
mod rerank;
pub use cache::model_cache_root;
pub use rerank::{RerankResult, rerank_resilient};

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("inference HTTP status {0}")]
    Http(u16),
    #[error("invalid inference response: {0}")]
    Invalid(&'static str),
    #[error("input exceeds model token limit")]
    TooLong,
    #[error("inference transport failed or timed out")]
    Transport,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub embeddings: bool,
    pub reranking: bool,
    pub token_counting: bool,
    pub batching: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub id: String,
    pub model: String,
    pub revision: String,
    pub sha256: String,
    pub engine_contract: String,
    pub dimensions: usize,
    pub max_tokens: usize,
    pub pooling: String,
    pub normalization: String,
    pub query_prefix: String,
    pub document_prefix: String,
    pub tokenizer: Artifact,
    pub capabilities: ModelCapabilities,
}
pub fn gemma_profile(dimensions: usize) -> Result<ModelProfile> {
    if ![256, 768].contains(&dimensions) {
        bail!("EmbeddingGemma 2 dimensions must be 256 or 768");
    }
    Ok(ModelProfile {
        id: format!("gemma2-bf16-llama-v1-{dimensions}"),
        model: "ggml-org/embeddinggemma-2-GGUF".into(),
        revision: "58faf90940886df2d62be555c8765d00dac362c5".into(),
        sha256: "68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216".into(),
        engine_contract: "embeddinggemma2-llama-b11869-mean-v1".into(),
        dimensions,
        max_tokens: 8192,
        pooling: "mean_including_prompt".into(),
        normalization: "truncate_then_l2".into(),
        query_prefix: "task: search result | query: ".into(),
        document_prefix: "title: none | text: ".into(),
        tokenizer: model_artifacts()[2].clone(),
        capabilities: ModelCapabilities {
            embeddings: true,
            reranking: false,
            token_counting: true,
            batching: true,
        },
    })
}
pub fn qwen_reranker_profile() -> ModelProfile {
    let artifact = model_artifacts()[1].clone();
    ModelProfile {
        id: "qwen3-reranker-0.6b-q8_0-llama-b11869-v1".into(),
        model: "ggml-org/Qwen3-Reranker-0.6B-Q8_0-GGUF".into(),
        revision: artifact.revision.clone(), sha256: artifact.sha256.clone(),
        engine_contract: "qwen3-reranker-llama-b11869-rank-v1".into(),
        dimensions: 0, max_tokens: 8192, pooling: "rank".into(), normalization: "none".into(),
        query_prefix: "Retrieve past LLM conversation turns containing the evidence needed to answer this history question.\nQuestion: ".into(),
        document_prefix: String::new(),
        // The tokenizer is embedded in the checksum-pinned GGUF.
        tokenizer: artifact,
        capabilities: ModelCapabilities { embeddings: false, reranking: true, token_counting: true, batching: true },
    }
}
pub trait TokenCounter {
    fn token_count(&mut self, text: &str) -> Result<usize>;
    fn set_deadline(&mut self, _deadline: Instant) {}
}
pub trait Embedder: TokenCounter {
    fn profile(&self) -> &ModelProfile;
    fn embed(&mut self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>>;
}
pub trait Reranker {
    fn profile(&self) -> &ModelProfile;
    fn rerank(&mut self, query: &str, documents: &[String]) -> Result<Vec<(usize, f64)>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub license: String,
    pub revision: String,
}
pub fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}
/// Returns a cached artifact, downloading it only when allowed. SHA-256 is checked
/// once, on download; `verify_artifacts` re-checks the cache on request.
pub fn artifact_path(a: &Artifact, allow_download: bool) -> Result<PathBuf> {
    cache::artifact_path_in(&model_cache_root(), a, allow_download)
}
/// Re-hashes cached models and inference files against their download pins.
pub fn verify_artifacts(cache: &Path) -> Result<Value> {
    verify_artifacts_in(cache, &model_cache_root())
}
fn verify_artifacts_in(cache: &Path, hub: &Path) -> Result<Value> {
    let manifest = native_manifest();
    let mut expected: Vec<(String, PathBuf, String)> = model_artifacts()
        .into_iter()
        .map(|a| {
            let path =
                cache::artifact_path_in(hub, &a, false).unwrap_or_else(|_| hub.join(&a.name));
            (a.name, path, a.sha256)
        })
        .collect();
    for f in manifest["llama_files"].as_array().into_iter().flatten() {
        expected.push((
            f["name"].as_str().unwrap_or_default().into(),
            cache.join(f["name"].as_str().unwrap_or_default()),
            f["sha256"].as_str().unwrap_or_default().into(),
        ));
    }
    let mut checked = 0;
    let mut absent = Vec::new();
    for (name, path, hash) in expected {
        if !path.is_file() {
            absent.push(name);
        } else if sha256(&path)? != hash {
            bail!("artifact checksum mismatch: {name}");
        } else {
            checked += 1;
        }
    }
    Ok(json!({"verified": checked, "absent": absent}))
}
pub fn model_artifacts() -> Vec<Artifact> {
    vec![Artifact {name:"embeddinggemma-2-BF16.gguf".into(),url:"https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/58faf90940886df2d62be555c8765d00dac362c5/embeddinggemma-2-BF16.gguf".into(),sha256:"68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216".into(),license:"Apache-2.0".into(),revision:"58faf90940886df2d62be555c8765d00dac362c5".into()},
        Artifact {name:"qwen3-reranker-0.6b-q8_0.gguf".into(),url:"https://huggingface.co/ggml-org/Qwen3-Reranker-0.6B-Q8_0-GGUF/resolve/a02f48bb4f057028298c21fa033da2b30d7742d5/qwen3-reranker-0.6b-q8_0.gguf".into(),sha256:"22c9979ce4fbcdc5acdc310c6641c32797eff1aa980b8f7a2db8a8ea23429a48".into(),license:"Apache-2.0".into(),revision:"a02f48bb4f057028298c21fa033da2b30d7742d5".into()},
        Artifact {name:"embeddinggemma-2-tokenizer.json".into(),url:"https://huggingface.co/google/embeddinggemma-2/resolve/914f7f89142e33e77833254d9c9b90c3cef7303b/tokenizer.json".into(),sha256:"4d777ef5bdc1aa36227abdfb77c3e49e7b9c892d16e1b6bda41c393504828be4".into(),license:"Apache-2.0".into(),revision:"914f7f89142e33e77833254d9c9b90c3cef7303b".into()}]
}
pub fn native_manifest() -> Value {
    let manifest: Value = serde_json::from_str(include_str!("../../../artifacts.json"))
        .expect("checked-in artifact manifest");
    manifest
        .get("platforms")
        .and_then(|p| p.get(host_platform()))
        .cloned()
        .unwrap_or(manifest)
}
pub fn host_platform() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x64"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x64"
    } else {
        "unsupported"
    }
}
/// The installed llama-server, when every pinned inference file is present.
pub fn native_server(cache: &Path) -> Result<PathBuf> {
    let manifest = native_manifest();
    if host_platform() == "unsupported" {
        bail!("no managed artifact pin for this host; configure a compatible remote endpoint");
    }
    let mut binary = None;
    for f in manifest["llama_files"]
        .as_array()
        .context("missing executable manifest")?
    {
        let name = f["name"].as_str().context("invalid filename")?;
        let path = cache.join(name);
        if !path.is_file() {
            bail!("inference file {name} is absent; run native artifact setup");
        }
        if name == "llama-server" || name == "llama-server.exe" {
            binary = Some(path);
        }
    }
    binary.context("llama-server is absent; run native artifact setup")
}
pub struct HttpEngine {
    deadline: Option<Instant>,
    request_timeout: Duration,
    client: Client,
    endpoint: String,
    key: Option<String>,
    model: String,
    profile: ModelProfile,
    tokenizer: Option<tokenizers::Tokenizer>,
}
impl HttpEngine {
    pub fn new(
        endpoint: &str,
        key: Option<String>,
        model: &str,
        profile: ModelProfile,
        timeout: Duration,
    ) -> Result<Self> {
        let url = reqwest::Url::parse(endpoint)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("endpoint must use HTTP(S) without embedded credentials");
        }
        Ok(Self {
            client: Client::builder().timeout(timeout).build()?,
            endpoint: endpoint.trim_end_matches('/').into(),
            key,
            model: model.into(),
            profile,
            tokenizer: None,
            deadline: None,
            request_timeout: timeout,
        })
    }
    pub fn with_tokenizer(mut self, path: &Path) -> Result<Self> {
        self.load_tokenizer(path)?;
        Ok(self)
    }
    pub fn load_tokenizer(&mut self, path: &Path) -> Result<()> {
        let mut tokenizer = tokenizers::Tokenizer::from_file(path)
            .map_err(|e| anyhow::anyhow!("tokenizer load: {e}"))?;
        tokenizer
            .with_truncation(None)
            .map_err(|e| anyhow::anyhow!("tokenizer configuration: {e}"))?;
        tokenizer.with_padding(None);
        self.tokenizer = Some(tokenizer);
        Ok(())
    }
    fn post(&self, path: &str, payload: Value) -> Result<Value> {
        let mut req = self
            .client
            .post(format!("{}{path}", self.endpoint))
            .json(&payload);
        if let Some(deadline) = self.deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(InferenceError::Transport.into());
            }
            req = req.timeout(remaining.min(self.request_timeout));
        }
        if let Some(k) = &self.key {
            req = req.bearer_auth(k);
        }
        let response = req.send().map_err(|_| InferenceError::Transport)?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.json::<Value>().ok();
            let message = body
                .as_ref()
                .and_then(|v| v.pointer("/error/message").or_else(|| v.get("message")))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if status == 413
                || ((400..=599).contains(&status)
                    && (message.contains("too large to process")
                        || message.contains("context length")
                        || message.contains("token limit")
                        || message.contains("physical batch size")))
            {
                return Err(InferenceError::TooLong.into());
            }
            return Err(InferenceError::Http(status).into());
        }
        Ok(response.json()?)
    }
}
impl TokenCounter for HttpEngine {
    fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = Some(deadline);
    }
    fn token_count(&mut self, text: &str) -> Result<usize> {
        // The pinned local tokenizer matches the server's; it avoids a round trip per count.
        if let Some(tokenizer) = &self.tokenizer {
            return Ok(tokenizer
                .encode(text, true)
                .map_err(|e| anyhow::anyhow!("tokenization failed: {e}"))?
                .get_ids()
                .len());
        }
        let r = match self.post("/tokenize", json!({"content":text,"add_special":true})) {
            Ok(r) => r,
            Err(e)
                if e.downcast_ref::<InferenceError>()
                    .is_some_and(|e| matches!(e, InferenceError::Http(400 | 404 | 405 | 422))) =>
            {
                let tokenizer = self.tokenizer.as_ref().context(
                    "endpoint has no tokenizer; explicitly fetch the pinned local tokenizer",
                )?;
                return Ok(tokenizer
                    .encode(text, true)
                    .map_err(|e| anyhow::anyhow!("tokenization failed: {e}"))?
                    .get_ids()
                    .len());
            }
            Err(e) => return Err(e),
        };
        Ok(r["tokens"]
            .as_array()
            .context("tokenizer endpoint must return token IDs")?
            .len())
    }
}
impl Embedder for HttpEngine {
    fn profile(&self) -> &ModelProfile {
        &self.profile
    }
    fn embed(&mut self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>> {
        let prefix = if query {
            &self.profile.query_prefix
        } else {
            &self.profile.document_prefix
        };
        let prompts: Vec<_> = texts.iter().map(|s| format!("{prefix}{s}")).collect();
        for prompt in &prompts {
            if self.token_count(prompt)? > self.profile.max_tokens {
                return Err(InferenceError::TooLong.into());
            }
        }
        let r = self.post(
            "/v1/embeddings",
            json!({"input":prompts,"model":self.model,"encoding_format":"float"}),
        )?;
        let mut items = r["data"]
            .as_array()
            .context("missing embedding data")?
            .clone();
        items.sort_by_key(|x| x["index"].as_u64().unwrap_or(u64::MAX));
        if items.len() != texts.len() {
            bail!("embedding response count mismatch");
        }
        items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if item["index"].as_u64() != Some(i as u64) {
                    bail!("embedding response indices are invalid");
                }
                let v: Vec<f32> = serde_json::from_value(item["embedding"].clone())?;
                normalize(&v, self.profile.dimensions)
            })
            .collect()
    }
}
impl Reranker for HttpEngine {
    fn profile(&self) -> &ModelProfile {
        &self.profile
    }

    fn rerank(&mut self, query: &str, documents: &[String]) -> Result<Vec<(usize, f64)>> {
        let r = self.post(
            "/v1/rerank",
            json!({"query":query,"documents":documents,"model":self.model,"top_n":documents.len()}),
        )?;
        let items = r
            .get("results")
            .or_else(|| r.get("data"))
            .and_then(Value::as_array)
            .context("missing reranking results")?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for item in items {
            let index = item["index"].as_u64().context("missing reranker index")? as usize;
            let score = item["relevance_score"]
                .as_f64()
                .context("missing reranker score")?;
            if index >= documents.len() || !score.is_finite() || !seen.insert(index) {
                bail!("invalid reranker result");
            }
            out.push((index, score));
        }
        if seen.len() != documents.len() {
            bail!("reranker omitted candidates");
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(out)
    }
}
pub fn normalize(v: &[f32], dimensions: usize) -> Result<Vec<f32>> {
    if v.len() < dimensions || dimensions == 0 || v.iter().any(|x| !x.is_finite()) {
        bail!("invalid embedding dimensions or non-finite value");
    }
    let norm = v[..dimensions]
        .iter()
        .map(|x| (*x as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    if norm <= 0.0 {
        bail!("zero embedding vector");
    }
    Ok(v[..dimensions]
        .iter()
        .map(|x| (*x as f64 / norm) as f32)
        .collect())
}
/// Splits text into chunks that fit with the prefix; each chunk carries its prefixed token count.
pub fn split_for_tokens(
    counter: &mut dyn TokenCounter,
    text: &str,
    prefix: &str,
    max_tokens: usize,
) -> Result<Vec<(String, usize)>> {
    fn split(
        c: &mut dyn TokenCounter,
        s: &str,
        prefix: &str,
        max: usize,
        out: &mut Vec<(String, usize)>,
    ) -> Result<()> {
        let tokens = c.token_count(&format!("{prefix}{s}"))?;
        if tokens <= max {
            out.push((s.into(), tokens));
            return Ok(());
        }
        let chars: Vec<_> = s.char_indices().map(|(i, _)| i).collect();
        if chars.len() < 2 {
            return Err(InferenceError::TooLong.into());
        }
        let middle = chars[chars.len() / 2];
        let at = s[..middle]
            .rfind('\n')
            .filter(|i| *i > middle / 2)
            .map(|i| i + 1)
            .unwrap_or(middle);
        split(c, &s[..at], prefix, max, out)?;
        split(c, &s[at..], prefix, max, out)
    }
    let mut out = Vec::new();
    split(counter, text, prefix, max_tokens, &mut out)?;
    Ok(out)
}

/// Owns only process lifecycle; the inference interface remains usable in-process.
pub struct ManagedLlama {
    child: Arc<Mutex<Child>>,
    stopped: Arc<AtomicBool>,
    monitor: Option<std::thread::JoinHandle<()>>,
    pub engine: HttpEngine,
    log: tempfile::NamedTempFile,
}
impl ManagedLlama {
    pub fn start(
        binary: &Path,
        model: &Path,
        reranking: bool,
        profile: ModelProfile,
        deadline: Duration,
        cancellation: CancellationToken,
    ) -> Result<Self> {
        let binary = binary.canonicalize()?;
        let model = model.canonicalize()?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let log = tempfile::NamedTempFile::new()?;
        let key = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut command = Command::new(&binary);
        // Non-causal embedding pooling needs each complete input to fit in a
        // physical batch. Use the same token limit as document chunking.
        let physical_batch = if reranking { 512 } else { profile.max_tokens }.to_string();
        command.current_dir(binary.parent().context("inference binary has no parent")?);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", binary.parent().unwrap());
        command
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--model",
            ])
            .arg(model)
            .args([
                "--api-key",
                &key,
                "--ctx-size",
                "8192",
                "--parallel",
                "1",
                "--batch-size",
                "8192",
                "--ubatch-size",
                &physical_batch,
                "--no-webui",
            ]);
        if reranking {
            command.args(["--reranking", "--pooling", "rank"]);
        } else {
            command.args(["--embeddings", "--pooling", "mean"]);
        }
        let engine = HttpEngine::new(
            &format!("http://127.0.0.1:{port}"),
            Some(key.clone()),
            "local",
            profile,
            deadline,
        )?;
        command
            .env_remove("DYLD_INSERT_LIBRARIES")
            .env_remove("LD_PRELOAD");
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.reopen()?))
            .stderr(Stdio::from(log.reopen()?))
            .spawn()
            .context("start verified llama-server")?;
        let mut server = Self {
            child: Arc::new(Mutex::new(child)),
            stopped: Arc::new(AtomicBool::new(false)),
            monitor: None,
            engine,
            log,
        };
        let child = Arc::clone(&server.child);
        let stopped = Arc::clone(&server.stopped);
        server.monitor = Some(
            std::thread::Builder::new()
                .name("llama-cancellation".into())
                .spawn(move || {
                    while !stopped.load(Ordering::Relaxed) {
                        if cancellation.is_cancelled() {
                            if let Ok(mut child) = child.lock() {
                                let _ = child.kill();
                            }
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                })?,
        );
        let start = Instant::now();
        let health = Client::builder()
            .timeout(Duration::from_millis(500))
            .build()?;
        loop {
            if server
                .child
                .lock()
                .map_err(|_| anyhow::anyhow!("process lock poisoned"))?
                .try_wait()?
                .is_some()
            {
                bail!(
                    "llama-server exited during startup{}",
                    log_tail(&server.log, &key)
                );
            }
            if health
                .get(format!("http://127.0.0.1:{port}/health"))
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            if start.elapsed() >= deadline {
                bail!(
                    "llama-server startup deadline exceeded{}",
                    log_tail(&server.log, &key)
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(server)
    }
}
/// The end of the managed server log, with the API key removed, for startup errors.
/// The log file is deleted with the server, so this is the only copy callers see.
fn log_tail(log: &tempfile::NamedTempFile, key: &str) -> String {
    const TAIL: u64 = 2048;
    let mut bytes = Vec::new();
    let Ok(mut file) = log.reopen() else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    if file
        .seek(SeekFrom::Start(len.saturating_sub(TAIL)))
        .is_err()
        || file.take(TAIL).read_to_end(&mut bytes).is_err()
    {
        return String::new();
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if len > TAIL {
        // Drop the partial first line, which may hold a fragment of the key.
        text = text
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .to_owned();
    }
    let text = text.replace(key, "[REDACTED]");
    let text = text.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("; log tail:\n{text}")
    }
}
impl Drop for ManagedLlama {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        {
            let mut child = self
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_rejects_corrupt_vectors() {
        assert!(normalize(&[0., 0.], 2).is_err());
        assert!(normalize(&[f32::NAN], 1).is_err());
        assert_eq!(normalize(&[3., 4., 8.], 2).unwrap(), vec![0.6, 0.8]);
    }
    struct Counter;
    impl TokenCounter for Counter {
        fn token_count(&mut self, s: &str) -> Result<usize> {
            Ok(s.chars().count())
        }
    }
    #[test]
    fn chunking_preserves_unicode_and_prefix_budget() -> Result<()> {
        let s = "🦀abcdefgh\n中文longinput";
        let chunks = split_for_tokens(&mut Counter, s, "query:", 12)?;
        assert_eq!(
            chunks.iter().map(|(c, _)| c.as_str()).collect::<String>(),
            s
        );
        assert!(chunks.iter().all(|(s, _)| s.chars().count() <= 6));
        assert!(chunks.iter().all(|(s, n)| *n == s.chars().count() + 6));
        Ok(())
    }
    #[test]
    fn artifacts_never_download_implicitly() -> Result<()> {
        let d = tempfile::tempdir()?;
        assert!(
            cache::artifact_path_in(&d.path().join("hub"), &model_artifacts()[0], false).is_err()
        );
        assert!(!d.path().join(&model_artifacts()[0].name).exists());
        Ok(())
    }
    #[test]
    fn verify_rejects_tampered_cache() -> Result<()> {
        let d = tempfile::tempdir()?;
        let a = model_artifacts().remove(0);
        let hub = d.path().join("hub");
        let repository = hf_hub::Repo::with_revision(
            "ggml-org/embeddinggemma-2-GGUF".into(),
            hf_hub::RepoType::Model,
            a.revision.clone(),
        );
        let path = hf_hub::Cache::new(hub.clone())
            .repo(repository)
            .pointer_path(&a.revision)
            .join(&a.name);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, "tampered")?;
        assert!(verify_artifacts_in(d.path(), &d.path().join("hub")).is_err());
        Ok(())
    }
}

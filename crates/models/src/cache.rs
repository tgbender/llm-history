use super::*;
use hf_hub::{Cache, Repo, RepoType, api::tokio::ApiBuilder};
use std::ffi::OsString;

fn hf_home(get: &impl Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(home) = get("HF_HOME") {
        return PathBuf::from(home);
    }
    get("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            get("HOME")
                .or_else(|| get("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cache")
        })
        .join("huggingface")
}
fn cache_root(get: &impl Fn(&str) -> Option<OsString>) -> PathBuf {
    get("HF_HUB_CACHE")
        .or_else(|| get("HUGGINGFACE_HUB_CACHE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| hf_home(get).join("hub"))
}
pub fn model_cache_root() -> PathBuf {
    cache_root(&|name| std::env::var_os(name))
}
fn location(a: &Artifact) -> Result<(Repo, &str)> {
    let (repository, rest) = a
        .url
        .strip_prefix("https://huggingface.co/")
        .and_then(|url| url.split_once("/resolve/"))
        .context("expected a Hugging Face model URL")?;
    let (revision, filename) = rest.split_once('/').context("model URL needs a filename")?;
    if revision != a.revision || a.sha256.len() != 64 || a.name.contains(['/', '\\']) {
        bail!("invalid model artifact");
    }
    Ok((
        Repo::with_revision(repository.into(), RepoType::Model, revision.into()),
        filename,
    ))
}
pub(super) fn artifact_path_in(hub: &Path, a: &Artifact, allow_download: bool) -> Result<PathBuf> {
    let (repo, filename) = location(a)?;
    let cached_repo = Cache::new(hub.to_owned()).repo(repo.clone());
    // Python caches pinned revisions directly, without a refs/<commit> file.
    let snapshot = cached_repo.pointer_path(&a.revision).join(filename);
    if snapshot.is_file() {
        return Ok(snapshot);
    }
    if !allow_download {
        bail!(
            "model {} is absent; run models fetch --download-model",
            a.name
        );
    }
    if std::env::var("HF_HUB_OFFLINE")
        .is_ok_and(|s| matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
    {
        bail!("model {} is absent and HF_HUB_OFFLINE is enabled", a.name);
    }
    let token = std::env::var("HF_TOKEN").ok().or_else(|| {
        let token_path = std::env::var_os("HF_TOKEN_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| hf_home(&|name| std::env::var_os(name)).join("token"));
        std::fs::read_to_string(token_path)
            .ok()
            .map(|s| s.trim().to_owned())
    });
    fetch(
        a,
        hub,
        repo,
        filename,
        ApiBuilder::from_env().with_token(token),
    )
}
fn fetch(
    a: &Artifact,
    hub: &Path,
    repo: Repo,
    filename: &str,
    builder: ApiBuilder,
) -> Result<PathBuf> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let path = runtime.block_on(async {
        let api = builder
            .with_cache_dir(hub.to_owned())
            .with_progress(false)
            .build()?;
        Ok::<_, anyhow::Error>(
            tokio::time::timeout(Duration::from_secs(600), api.repo(repo).get(filename))
                .await
                .context("model download timed out")??,
        )
    })?;
    // Cached paths never take this full-file read.
    if sha256(&path)? != a.sha256 {
        let _ = std::fs::remove_file(&path);
        bail!("downloaded model checksum mismatch: {}", a.name);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_cache_environment_precedence() {
        let mut env = std::collections::HashMap::from([("HOME", OsString::from("/user"))]);
        assert_eq!(
            cache_root(&|key| env.get(key).cloned()),
            PathBuf::from("/user/.cache/huggingface/hub")
        );
        env.insert("XDG_CACHE_HOME", "/xdg".into());
        assert_eq!(
            cache_root(&|key| env.get(key).cloned()),
            PathBuf::from("/xdg/huggingface/hub")
        );
        env.insert("HF_HOME", "/hf".into());
        assert_eq!(
            cache_root(&|key| env.get(key).cloned()),
            PathBuf::from("/hf/hub")
        );
        env.insert("HF_HUB_CACHE", "/hub".into());
        assert_eq!(
            cache_root(&|key| env.get(key).cloned()),
            PathBuf::from("/hub")
        );
    }
    #[test]
    fn reuses_pinned_snapshots_without_refs_or_rehashing() -> Result<()> {
        let d = tempfile::tempdir()?;
        for artifact in model_artifacts() {
            let (repo, filename) = location(&artifact)?;
            let path = Cache::new(d.path().to_owned())
                .repo(repo)
                .pointer_path(&artifact.revision)
                .join(filename);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, b"synthetic cached data")?;
            assert_eq!(artifact_path_in(d.path(), &artifact, false)?, path);
            assert_eq!(artifact_path_in(d.path(), &artifact, true)?, path);
        }
        Ok(())
    }
}

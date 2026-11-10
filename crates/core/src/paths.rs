//! Filesystem path resolution shared by application entry points and storage.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn database_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if absolute.exists() {
        return Ok(absolute.canonicalize()?);
    }
    let parent = absolute
        .parent()
        .context("database needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    Ok(parent
        .canonicalize()?
        .join(absolute.file_name().context("database needs a filename")?))
}

pub fn path_contains(root: &str, path: &str) -> bool {
    let root = root.replace('\\', "/");
    let path = path.replace('\\', "/");
    let (root, path) = if root.as_bytes().get(1) == Some(&b':') {
        (root.to_lowercase(), path.to_lowercase())
    } else {
        (root, path)
    };
    let root = root.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

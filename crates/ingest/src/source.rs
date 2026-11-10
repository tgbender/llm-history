//! Inventory and locators for supported native transcripts.
use anyhow::Result;
use llm_history_core::{is_compressed, logical_path, mtime_ns};
use llm_history_types::{Harness, Profile};
use std::path::{Path, PathBuf};

pub struct Transcript {
    pub path: PathBuf,
    pub key: String,
    pub(crate) harness: Harness,
    pub(crate) sqlite: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Observation {
    pub size: u64,
    pub mtime_ns: i64,
    pub token: String,
}

impl Transcript {
    pub fn observe(&self) -> Result<Observation> {
        let meta = self.path.metadata()?;
        let token = if self.sqlite {
            crate::cursor::source::snapshot_token(&self.path)?
        } else {
            String::new()
        };
        Ok(Observation {
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            token,
        })
    }
}

impl Observation {
    pub fn matches(&self, size: Option<u64>, mtime: Option<i64>, token: &str) -> bool {
        size == Some(self.size)
            && mtime == Some(self.mtime_ns)
            && (self.token.is_empty() || token == self.token)
    }
}

fn transcript_dirs(harness: Harness) -> &'static [&'static str] {
    match harness {
        Harness::Codex => &["sessions", "archived_sessions"],
        Harness::Claude => &["projects"],
        Harness::Cursor => &[],
    }
}

/// List transcript files and their stable keys in one inventory pass.
pub fn discover(profile: &Profile) -> Result<Vec<Transcript>> {
    if profile.harness == Harness::Cursor {
        return crate::cursor::source::discover(&profile.root);
    }
    fn walk(path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let path = entry.path();
            if ty.is_dir() {
                walk(&path, out)?;
            } else if ty.is_file()
                && (path.extension().is_some_and(|ext| ext == "jsonl")
                    // Prefer the live file over a compressed copy still being written.
                    || is_compressed(&path) && !logical_path(&path).exists())
            {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    for dir in transcript_dirs(profile.harness) {
        walk(&profile.root.join(dir), &mut paths)?;
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let key = logical_path(&path)
                .strip_prefix(&profile.root)?
                .to_string_lossy()
                .into_owned();
            Ok(Transcript {
                path,
                key,
                harness: profile.harness,
                sqlite: false,
            })
        })
        .collect()
}

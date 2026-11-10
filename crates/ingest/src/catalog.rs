//! Bounded native catalog acquisition and source change detection.
use crate::{claude_code::catalog::claude_catalog, codex::catalog::codex_catalog};
use anyhow::{Result, bail};
use llm_history_core::mtime_ns;
use llm_history_types::{Harness, Profile, catalog::ProjectCatalog};
use serde_json::Value;
use std::{fs::File, io::Read, path::PathBuf};
fn json_catalog(path: &std::path::Path) -> Result<Value> {
    let mut file = File::open(path)?;
    let before = file.metadata()?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        bail!("catalog exceeds 32 MiB");
    }
    let after = file.metadata()?;
    let published = path.metadata()?;
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || after.len() != published.len()
        || after.modified()? != published.modified()?
    {
        bail!("catalog changed during capture");
    }
    Ok(serde_json::from_slice(&bytes)?)
}
pub fn load_catalog(profile: &Profile) -> Result<Option<(PathBuf, &'static str, ProjectCatalog)>> {
    if profile.harness == Harness::Codex {
        let mut sqlite_failed = false;
        for path in [
            profile.root.join("state_5.sqlite"),
            profile.root.join("sqlite/state_5.sqlite"),
        ] {
            if path.is_file() {
                match crate::codex::source::read_project_catalog(&path) {
                    Ok(Some(catalog)) => return Ok(Some((path, "codex-state-sqlite", catalog))),
                    Ok(None) => {}
                    Err(_) => sqlite_failed = true,
                }
            }
        }
        let path = profile.root.join(".codex-global-state.json");
        if path.is_file()
            && let Some(catalog) = codex_catalog(&json_catalog(&path)?)
        {
            return Ok(Some((path, "codex-desktop-global-state", catalog)));
        }
        if sqlite_failed {
            bail!("SQLite catalog could not be read");
        }
    } else if profile.harness == Harness::Claude {
        let path = PathBuf::from(format!("{}.json", profile.root.to_string_lossy()));
        if path.is_file()
            && let Some(catalog) = claude_catalog(&json_catalog(&path)?)
        {
            return Ok(Some((path, "claude-global-config", catalog)));
        }
    }
    Ok(None)
}
// Observe the SQLite WAL too: the main file can stay unchanged across commits.
pub fn catalog_fingerprint(profile: &Profile) -> Result<String> {
    let paths = if profile.harness == Harness::Codex {
        vec![
            profile.root.join("state_5.sqlite"),
            profile.root.join("sqlite/state_5.sqlite"),
            profile.root.join(".codex-global-state.json"),
        ]
    } else if profile.harness == Harness::Claude {
        vec![PathBuf::from(format!(
            "{}.json",
            profile.root.to_string_lossy()
        ))]
    } else {
        Vec::new()
    };
    let mut observed = Vec::new();
    for path in paths {
        let wal = PathBuf::from(format!("{}-wal", path.to_string_lossy()));
        for file in [path, wal] {
            let state = match file.metadata() {
                Ok(meta) => Some((meta.len(), mtime_ns(&meta))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            observed.push((file, state));
        }
    }
    Ok(serde_json::to_string(&observed)?)
}

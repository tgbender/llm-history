use super::*;

fn fingerprint(path: &Path) -> Result<(u64, i64)> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.len(), llm_history_core::mtime_ns(&meta)))
}
fn sync_parent(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(_path.parent().context("archive needs a parent directory")?)?.sync_all()?;
    Ok(())
}
fn inspect(store: &Store, deep: bool) -> Result<ArchiveInfo> {
    let schema = store.require_schema()?;
    let wal = store.path.with_file_name(format!(
        "{}.wal",
        store.path.file_name().unwrap().to_string_lossy()
    ));
    if wal.exists() && wal.metadata()?.len() > 0 {
        bail!("archive has an outstanding WAL; create a checkpointed archive first");
    }
    let raw = if deep { store.verify()? } else { Value::Null };
    let bytes = std::fs::metadata(&store.path)?.len();
    Ok(ArchiveInfo {
        archive: store.path.clone(),
        identity: store.identity()?,
        schema,
        bytes,
        raw,
    })
}
pub fn verify_archive(path: &Path) -> Result<ArchiveInfo> {
    inspect(&Store::open(path, true)?, true)
}
fn candidate(
    source: &Path,
    destination: &Path,
    verified: Option<&ArchiveInfo>,
) -> Result<(tempfile::TempPath, ArchiveInfo)> {
    let temp = tempfile::Builder::new()
        .prefix(".history-publish-")
        .tempfile_in(destination.parent().context("destination needs a parent")?)?;
    std::fs::copy(source, temp.path())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o600))?;
    }
    temp.as_file().sync_all()?;
    // Windows refuses DuckDB the file while this writable handle is open.
    let temp = temp.into_temp_path();
    // This path is private and unpublished; the caller holds the destination lock.
    let verification = if let Some(verified) = verified {
        if std::fs::metadata(&temp)?.len() != verified.bytes {
            bail!("archive changed during copy");
        }
        verified.clone()
    } else {
        inspect(&Store::open_locked(&temp, true, None, false)?, false)?
    };
    Ok((temp, verification))
}
fn publish_copy(source: &Path, destination: &Path) -> Result<ArchiveInfo> {
    let _lock = DatabaseLock::acquire(destination, false, std::time::Duration::ZERO)?;
    if destination.exists() {
        bail!("archive destination already exists");
    }
    let (temp, mut info) = candidate(source, destination, None)?;
    temp.persist_noclobber(destination).map_err(|e| e.error)?;
    sync_parent(destination)?;
    info.archive = destination.to_owned();
    Ok(info)
}
impl Store {
    /// Rewrites live pages into a new file, then renames it over the old one. A
    /// failed copy leaves the original untouched.
    pub fn compact(self) -> Result<Value> {
        if self.read_only {
            bail!("compaction requires a writer connection");
        }
        self.database.connection().execute_batch("CHECKPOINT")?;
        let before = std::fs::metadata(&self.path)?.len();
        let source: String =
            self.database
                .connection()
                .query_row("SELECT current_database()", [], |r| r.get(0))?;
        // DuckDB 2.0 COPY omits triggers, including incremental FTS maintenance.
        let triggers = self
            .database
            .connection()
            .prepare(
                "SELECT sql \
                 FROM duckdb_triggers() \
                 WHERE database_name=current_database() AND NOT temporary \
                 ORDER BY schema_name,trigger_name",
            )?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let temporary = tempfile::Builder::new()
            .prefix(".history-compact-")
            .tempdir_in(self.path.parent().unwrap())?;
        let candidate = temporary.path().join(self.path.file_name().unwrap());
        let quoted = candidate.to_string_lossy().replace('\'', "''");
        self.database.connection().execute_batch(&format!(
            "ATTACH '{quoted}' AS compacted; COPY FROM DATABASE \"{}\" TO compacted;",
            source.replace('"', "\"\"")
        ))?;
        // Restore triggers using the connection that already loaded FTS/ICU.
        self.database.connection().execute_batch("USE compacted")?;
        for trigger in triggers {
            self.database.connection().execute_batch(&trigger)?;
        }
        self.database.connection().execute_batch(&format!(
            "CHECKPOINT compacted; USE \"{}\"; DETACH compacted",
            source.replace('"', "\"\"")
        ))?;
        // Windows flushes only through a handle opened for writing.
        OpenOptions::new()
            .write(true)
            .open(&candidate)?
            .sync_all()?;
        std::fs::set_permissions(&candidate, std::fs::metadata(&self.path)?.permissions())?;
        let after = std::fs::metadata(&candidate)?.len();
        let Store { database, path, .. } = self;
        let lock = database.close();
        std::fs::rename(candidate, &path)?;
        sync_parent(&path)?;
        drop(lock);
        Ok(
            json!({"database":path,"before_bytes":before,"after_bytes":after,"saved_bytes":before.saturating_sub(after)}),
        )
    }
    /// Copies the checkpointed database file to `to`. The store is closed first because
    /// Windows lets nothing else read a database DuckDB holds open for writing; the
    /// writer lock stays held until the copy is published.
    pub fn snapshot(self, to: &Path) -> Result<ArchiveInfo> {
        if self.read_only {
            bail!("archive creation requires the writer lock");
        }
        let to = database_path(to)?;
        self.database.connection().execute_batch("CHECKPOINT")?;
        let Store { database, path, .. } = self;
        let _lock = database.close();
        publish_copy(&path, &to)
    }
}
pub fn restore_archive(
    archive: &Path,
    destination: &Path,
    replace: bool,
) -> Result<RestoreReceipt> {
    let source = Store::open(archive, true)?;
    let destination = database_path(destination)?;
    if source.path == destination {
        bail!("archive and destination must differ");
    }
    let _ingestion = Store::lock_ingestion(&destination, std::time::Duration::ZERO)?;
    let _lock = DatabaseLock::acquire(&destination, false, std::time::Duration::ZERO)?;
    if destination.exists() && !replace {
        bail!(
            "restore over an existing database requires --yes; the current database will be archived first"
        );
    }
    // Inspect schema/WAL before publication; full payload checks are explicit.
    let verified = inspect(&source, false)?;
    let (temp, mut verification) = candidate(&source.path, &destination, Some(&verified))?;
    let archived_previous = if destination.exists() {
        let current = Store::open_locked(&destination, false, None, false)?;
        current.database.connection().execute_batch("CHECKPOINT")?;
        // Close DuckDB first: Windows lets nothing else read or replace a database it
        // holds open for writing. `_lock` still excludes other llm-history processes.
        drop(current);
        let before = fingerprint(&destination)?;
        let directory = destination.with_file_name(format!(
            "{}.archives",
            destination.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&directory)?;
        let backup = directory.join(format!("pre-restore-{}.duckdb", uuid::Uuid::new_v4()));
        publish_copy(&destination, &backup)?;
        if fingerprint(&destination)? != before {
            bail!("active database changed while archiving; restore aborted");
        }
        Some(backup)
    } else {
        None
    };
    if destination.exists() {
        temp.persist(&destination).map_err(|e| e.error)?;
    } else {
        temp.persist_noclobber(&destination).map_err(|e| e.error)?;
    }
    sync_parent(&destination)?;
    verification.archive = destination.clone();
    Ok(RestoreReceipt {
        restored: destination,
        restored_from: source.path.clone(),
        archived_previous,
        verification,
    })
}

pub fn list_archives(directory: &Path) -> Result<Value> {
    if !directory.exists() {
        return Ok(json!([]));
    }
    let mut paths = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    let mut rows = Vec::new();
    for path in paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "duckdb") && p.is_file())
        .take(1000)
    {
        let meta = std::fs::metadata(&path)?;
        rows.push(json!({"archive":path,"bytes":meta.len()}));
    }
    Ok(json!(rows))
}

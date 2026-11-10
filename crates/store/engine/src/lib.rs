//! Configured DuckDB connections and their lock/transaction lifetimes.
//!
//! Schema and history semantics belong to the storage clients. Every connection
//! loads our pinned native extensions and registers embedded hash functions
//! before queries. Raw driver access is a seam for storage implementations;
//! application callers use history operations.
use anyhow::{Context, Result, bail};
use duckdb::{Connection, Transaction};
use llm_history_types::storage::{DatabaseBusy, StoreOptions};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    time::{Duration, Instant},
};

mod hashing;

pub struct DatabaseLock {
    _file: File,
}

impl DatabaseLock {
    pub fn acquire(path: &Path, read_only: bool, timeout: Duration) -> Result<Self> {
        Self::acquire_file(&path.with_extension("duckdb.lock"), read_only, timeout)
    }

    pub fn acquire_ingestion(path: &Path, timeout: Duration) -> Result<Self> {
        Self::acquire_file(&path.with_extension("duckdb.ingest.lock"), false, timeout)
    }

    fn acquire_file(path: &Path, read_only: bool, timeout: Duration) -> Result<Self> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let deadline = Instant::now() + timeout;
        loop {
            let result = if read_only {
                fs2::FileExt::try_lock_shared(&lock)
            } else {
                fs2::FileExt::try_lock_exclusive(&lock)
            };
            match result {
                Ok(()) => return Ok(Self { _file: lock }),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
                {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(DatabaseBusy.into());
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(50)));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

pub struct Database {
    connection: Connection,
    lock: Option<DatabaseLock>,
}

impl Database {
    /// The caller resolves the path and owns schema initialization. A supplied
    /// lock is retained through close; archive callers may already hold it.
    pub fn open_locked(
        path: &Path,
        read_only: bool,
        options: StoreOptions,
        lock: Option<DatabaseLock>,
    ) -> Result<Self> {
        register_extensions()?;
        let config = duckdb::Config::default().access_mode(if read_only {
            duckdb::AccessMode::ReadOnly
        } else {
            duckdb::AccessMode::ReadWrite
        })?;
        let connection = Connection::open_with_flags(path, config)?;
        #[cfg(unix)]
        if !read_only {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        connection.execute_batch("SET autoload_known_extensions=false; SET autoinstall_known_extensions=false; SET allow_unsigned_extensions=false; LOAD icu; LOAD fts; SET TimeZone='UTC';")?;
        connection.execute_batch(&format!("SET memory_limit='{}MiB'", options.memory_mib))?;
        hashing::register(&connection).context("register bundled DuckDB hash functions")?;
        Ok(Self { connection, lock })
    }

    /// Interrupt a running query at its deadline and join the watchdog before
    /// letting the connection serve another query.
    pub fn with_query_timeout<T>(
        &self,
        timeout: Duration,
        run: impl FnOnce(Instant) -> Result<T>,
    ) -> Result<T> {
        if timeout.is_zero() {
            bail!("SQL timeout must be positive");
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .context("SQL timeout out of range")?;
        let interrupt = self.connection.interrupt_handle();
        std::thread::scope(|scope| {
            use std::sync::mpsc::{self, RecvTimeoutError::Timeout};

            let (finished, done) = mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("sql-deadline".into())
                .spawn_scoped(scope, move || {
                    if matches!(
                        done.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                        Err(Timeout)
                    ) {
                        // An interrupt during preparation or between engine calls
                        // may be cleared by the next call. Keep it armed until done.
                        loop {
                            interrupt.interrupt();
                            if !matches!(done.recv_timeout(Duration::from_millis(20)), Err(Timeout))
                            {
                                break;
                            }
                        }
                    }
                })
                .context("start SQL deadline watchdog")?;
            let result = run(deadline);
            let expired = Instant::now() >= deadline;
            drop(finished);
            if expired {
                bail!(
                    "SQL query timed out after {} seconds",
                    timeout.as_secs_f64()
                );
            }
            result
        })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// An explicit transaction token borrowed by all participating repositories.
    /// Nested transactions fail at BEGIN; dropping this token rolls back.
    pub fn begin(&self) -> Result<WriteTransaction<'_>> {
        Ok(WriteTransaction {
            transaction: self.connection.unchecked_transaction()?,
        })
    }

    /// Close the database while retaining its stable lock across file replacement.
    pub fn close(self) -> Option<DatabaseLock> {
        let Self { connection, lock } = self;
        drop(connection);
        lock
    }
}

pub struct WriteTransaction<'database> {
    transaction: Transaction<'database>,
}

impl WriteTransaction<'_> {
    pub fn connection(&self) -> &Connection {
        &self.transaction
    }
    pub fn commit(self) -> Result<()> {
        Ok(self.transaction.commit()?)
    }
}

fn register_extensions() -> Result<()> {
    unsafe extern "C" {
        fn llm_history_register_duckdb_extensions() -> i32;
    }
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !REGISTERED.get_or_init(|| unsafe { llm_history_register_duckdb_extensions() == 0 }) {
        bail!("could not register bundled DuckDB FTS/ICU extensions");
    }
    Ok(())
}

pub fn verify_linked_engine() -> Result<()> {
    let linked = unsafe { std::ffi::CStr::from_ptr(libduckdb_sys::duckdb_library_version()) }
        .to_str()
        .context("native DuckDB version")?;
    if linked != "v2.0.0" {
        bail!("expected native DuckDB v2.0.0; linked {linked}");
    }
    Ok(())
}

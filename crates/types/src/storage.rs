//! Database resource options and lock-contention error values.
pub const DEFAULT_SQL_TIMEOUT_SECONDS: u64 = 15;

#[derive(Debug, Clone, Copy)]
pub struct StoreOptions {
    pub memory_mib: u64,
    pub batch_records: usize,
    pub batch_bytes: usize,
}
impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            memory_mib: 1024,
            batch_records: 20_000,
            batch_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct DatabaseBusy;
impl std::fmt::Display for DatabaseBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("database is busy; retry after the other reader/writer finishes")
    }
}
impl std::error::Error for DatabaseBusy {}

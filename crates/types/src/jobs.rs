//! Durable indexing progress values.
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexReceipt {
    pub documents: i64,
    pub changed: bool,
    pub rebuilt: bool,
    pub mode: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkReceipt {
    pub status: String,
    pub embedded: usize,
    pub attempted: usize,
    pub merged: usize,
    pub failures: usize,
    pub tokens: usize,
    pub elapsed_seconds: f64,
    pub database_waits: usize,
    pub oversized: usize,
    pub queued_chunks: usize,
    pub queued_bytes: u64,
    pub pid: u32,
    pub model_profile: String,
    pub profiles: Vec<String>,
    pub updated_at: i64,
    pub next_launch_at: i64,
}

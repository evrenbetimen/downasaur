//! Byte-exact, resumable downloads.
//!
//! - [`chunks`]: splits a resource into ranged chunks and tracks per-chunk
//!   progress so a pause/crash resumes from the exact byte (state lives in SQLite).
//! - [`http`]: parallel ranged fetches, each through a large `BufWriter` so 8K
//!   streams hit the disk in big sequential blocks instead of many small writes.
//! - [`hls`]: HLS VOD segment fetch and live playlist following.
//! - [`rate`]: token-bucket bandwidth limiter shared by all workers.

pub mod chunks;
pub mod hls;
pub mod http;
pub mod rate;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use chunks::{Chunk, plan_chunks};

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Bytes per ranged chunk. Larger chunks mean fewer requests; smaller ones mean
    /// finer-grained resume and better load-spreading across connections.
    pub chunk_size: u64,
    /// Parallel connections per task.
    pub connections: usize,
    /// In-memory write buffer per connection before flushing to disk.
    pub buffer_capacity: usize,
    /// Persist chunk progress at most this often (bytes) to keep SQLite writes cheap.
    pub checkpoint_every: u64,
    /// Stop recording a live stream after this much media and finish the task
    /// normally (remux + organize). `None` records until the stream ends.
    pub live_max_duration: Option<std::time::Duration>,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            chunk_size: 32 * 1024 * 1024,
            connections: 8,
            buffer_capacity: 8 * 1024 * 1024,
            checkpoint_every: 4 * 1024 * 1024,
            live_max_duration: None,
        }
    }
}

/// Lock-free byte counter shared between workers and the progress reporter.
#[derive(Debug, Default, Clone)]
pub struct ByteCounter(Arc<AtomicU64>);

impl ByteCounter {
    pub fn new(initial: u64) -> Self {
        Self(Arc::new(AtomicU64::new(initial)))
    }

    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

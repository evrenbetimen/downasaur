//! Chunk planning and resume bookkeeping.

use serde::{Deserialize, Serialize};

/// An inclusive byte range `[start, end]` plus how many bytes of it are on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub index: u32,
    pub start: u64,
    pub end: u64,
    pub done: u64,
}

impl Chunk {
    pub const fn len(&self) -> u64 {
        self.end - self.start + 1
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub const fn is_complete(&self) -> bool {
        self.done >= self.len()
    }

    /// Next byte to request.
    pub const fn cursor(&self) -> u64 {
        self.start + self.done
    }

    /// `Range` header value for the remaining bytes.
    pub fn range_header(&self) -> String {
        format!("bytes={}-{}", self.cursor(), self.end)
    }
}

/// Split `total` bytes into chunks of at most `chunk_size`.
pub fn plan_chunks(total: u64, chunk_size: u64) -> Vec<Chunk> {
    let chunk_size = chunk_size.max(1);
    (0..total.div_ceil(chunk_size))
        .map(|i| {
            let start = i * chunk_size;
            Chunk {
                index: u32::try_from(i).unwrap_or(u32::MAX),
                start,
                end: (start + chunk_size).min(total) - 1,
                done: 0,
            }
        })
        .collect()
}

pub fn bytes_done(chunks: &[Chunk]) -> u64 {
    chunks.iter().map(|c| c.done.min(c.len())).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_cover_every_byte_exactly_once() {
        let chunks = plan_chunks(100, 30);
        assert_eq!(chunks.len(), 4);
        assert_eq!((chunks[0].start, chunks[0].end), (0, 29));
        assert_eq!((chunks[3].start, chunks[3].end), (90, 99));
        assert_eq!(chunks.iter().map(Chunk::len).sum::<u64>(), 100);
    }

    #[test]
    fn resume_range_starts_after_done_bytes() {
        let mut c = plan_chunks(100, 50)[1];
        c.done = 10;
        assert_eq!(c.range_header(), "bytes=60-99");
        assert!(!c.is_complete());
    }
}

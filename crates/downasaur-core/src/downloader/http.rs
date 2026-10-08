//! Parallel ranged HTTP downloader with buffered disk writes.

use std::io::SeekFrom;
use std::path::Path;

use futures::{StreamExt, TryStreamExt};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT_RANGES, CONTENT_LENGTH, RANGE};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::rate::RateLimiter;
use super::{ByteCounter, Chunk, DownloadOptions};
use crate::error::{CoreError, Result};
use crate::net::HttpPool;

const CHUNK_ATTEMPTS: u32 = 3;

/// Everything a ranged download needs; cheap to clone per worker.
#[derive(Debug, Clone)]
pub struct HttpJob<'a> {
    pub pool: &'a HttpPool,
    pub url: &'a Url,
    pub headers: &'a [(String, String)],
    pub dest: &'a Path,
    pub options: &'a DownloadOptions,
    pub limiter: &'a RateLimiter,
    pub counter: &'a ByteCounter,
    pub cancel: &'a CancellationToken,
}

/// Size and range support as reported by a `HEAD` (falls back to a 1-byte GET).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub content_length: Option<u64>,
    pub ranges: bool,
}

impl HttpJob<'_> {
    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        self.headers.iter().fold(self.pool.client().request(method, self.url.clone()), |r, (k, v)| r.header(k, v))
    }

    pub async fn probe(&self) -> Result<Probe> {
        let resp = self.pool.send_with_retry(|_| self.request(reqwest::Method::GET).header(RANGE, "bytes=0-0")).await?;
        if resp.status() == StatusCode::PARTIAL_CONTENT {
            // `Content-Range: bytes 0-0/12345`
            let total = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next())
                .and_then(|v| v.parse().ok());
            return Ok(Probe { content_length: total, ranges: true });
        }
        let ranges = resp.headers().get(ACCEPT_RANGES).and_then(|v| v.to_str().ok()) == Some("bytes");
        let content_length =
            resp.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
        Ok(Probe { content_length, ranges })
    }

    /// Download all incomplete `chunks` in parallel. `on_checkpoint` receives a
    /// chunk whenever its flushed progress advances, for persisting to the database.
    pub async fn run<F>(&self, chunks: Vec<Chunk>, on_checkpoint: F) -> Result<Vec<Chunk>>
    where
        F: Fn(Chunk) + Send + Sync,
    {
        self.preallocate(chunks.last().map_or(0, |c| c.end + 1)).await?;
        let on_checkpoint = &on_checkpoint;
        futures::stream::iter(chunks)
            .map(|c| async move { self.fetch_with_retry(c, on_checkpoint).await })
            .buffer_unordered(self.options.connections.max(1))
            .try_collect()
            .await
    }

    async fn preallocate(&self, len: u64) -> Result<()> {
        let file = OpenOptions::new().create(true).write(true).truncate(false).open(self.dest).await?;
        if file.metadata().await?.len() < len {
            file.set_len(len).await?;
        }
        Ok(())
    }

    async fn fetch_with_retry<F: Fn(Chunk)>(&self, mut chunk: Chunk, on_checkpoint: &F) -> Result<Chunk> {
        let mut attempt = 0;
        loop {
            match self.fetch_chunk(&mut chunk, on_checkpoint).await {
                Ok(()) => return Ok(chunk),
                Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                Err(e) if attempt + 1 < CHUNK_ATTEMPTS => {
                    tracing::warn!(chunk = chunk.index, attempt, error = %e, "chunk failed, resuming");
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn fetch_chunk<F: Fn(Chunk)>(&self, chunk: &mut Chunk, on_checkpoint: &F) -> Result<()> {
        if chunk.is_complete() {
            return Ok(());
        }
        let resp = self
            .pool
            .send_with_retry(|_| self.request(reqwest::Method::GET).header(RANGE, chunk.range_header()))
            .await?;
        if resp.status() != StatusCode::PARTIAL_CONTENT {
            return Err(CoreError::Parse(format!("server ignored Range request (HTTP {})", resp.status())));
        }

        let mut file = OpenOptions::new().write(true).open(self.dest).await?;
        file.seek(SeekFrom::Start(chunk.cursor())).await?;
        let mut writer = BufWriter::with_capacity(self.options.buffer_capacity, file);
        let mut stream = resp.bytes_stream();
        let mut unflushed = 0u64;

        let outcome: Result<()> = loop {
            let next = tokio::select! {
                () = self.cancel.cancelled() => break Err(CoreError::Cancelled),
                next = stream.next() => next,
            };
            match next {
                Some(Ok(bytes)) => {
                    let remaining = chunk.len() - chunk.done;
                    let take = usize::try_from(remaining).map_or(bytes.len(), |r| bytes.len().min(r));
                    self.limiter.acquire(take).await;
                    writer.write_all(&bytes[..take]).await?;
                    chunk.done += take as u64;
                    unflushed += take as u64;
                    self.counter.add(take as u64);
                    if unflushed >= self.options.checkpoint_every {
                        writer.flush().await?;
                        on_checkpoint(*chunk);
                        unflushed = 0;
                    }
                    if chunk.is_complete() {
                        break Ok(());
                    }
                }
                Some(Err(e)) => break Err(e.into()),
                None if chunk.is_complete() => break Ok(()),
                None => break Err(CoreError::Parse("connection closed before chunk completed".into())),
            }
        };

        // Always flush so the checkpoint never claims bytes that are not on disk.
        writer.flush().await?;
        on_checkpoint(*chunk);
        outcome
    }

    /// Fallback for servers without range support: single sequential stream.
    pub async fn run_sequential(&self) -> Result<u64> {
        let resp = self.pool.send_with_retry(|_| self.request(reqwest::Method::GET)).await?;
        let file = OpenOptions::new().create(true).write(true).truncate(true).open(self.dest).await?;
        let mut writer = BufWriter::with_capacity(self.options.buffer_capacity, file);
        let mut stream = resp.bytes_stream();
        let mut total = 0u64;
        loop {
            let next = tokio::select! {
                () = self.cancel.cancelled() => return Err(CoreError::Cancelled),
                next = stream.next() => next,
            };
            let Some(bytes) = next else { break };
            let bytes = bytes?;
            self.limiter.acquire(bytes.len()).await;
            writer.write_all(&bytes).await?;
            total += bytes.len() as u64;
            self.counter.add(bytes.len() as u64);
        }
        writer.flush().await?;
        Ok(total)
    }
}

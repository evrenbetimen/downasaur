//! HLS media-playlist downloader (VOD and live).
//!
//! Segments are fetched concurrently but written strictly in order through a
//! single buffered writer, producing one continuous MPEG-TS/fMP4 file that the
//! remux stage turns into MP4/MKV. Live playlists are re-polled every target
//! duration and new media-sequence numbers appended until the stream ends
//! (`#EXT-X-ENDLIST`) or the task is cancelled.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use m3u8_rs::MediaPlaylist;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::ByteCounter;
use super::rate::RateLimiter;
use crate::drm;
use crate::error::{CoreError, Result};
use crate::net::HttpPool;
use crate::net::fingerprint::DeviceClass;

#[derive(Debug)]
pub struct HlsJob<'a> {
    pub pool: &'a HttpPool,
    pub playlist_url: &'a Url,
    pub dest: &'a Path,
    pub parallel_segments: usize,
    pub buffer_capacity: usize,
    pub limiter: &'a RateLimiter,
    pub counter: &'a ByteCounter,
    pub cancel: &'a CancellationToken,
}

impl HlsJob<'_> {
    async fn load(&self) -> Result<MediaPlaylist> {
        let text = self.pool.get_text(self.playlist_url, DeviceClass::Desktop).await?;
        drm::ensure_clear(drm::detect_in_hls(&text))?;
        if text.contains("METHOD=AES-128") {
            return Err(CoreError::Parse("AES-128 encrypted HLS segments are not supported yet".into()));
        }
        m3u8_rs::parse_media_playlist_res(text.as_bytes())
            .map_err(|e| CoreError::Parse(format!("invalid HLS media playlist: {e}")))
    }

    pub async fn run(&self) -> Result<u64> {
        let file = OpenOptions::new().create(true).append(true).open(self.dest).await?;
        let mut writer = BufWriter::with_capacity(self.buffer_capacity, file);
        let mut seen = HashSet::<u64>::new();
        let mut written = 0u64;

        loop {
            let playlist = self.load().await?;
            let fresh: Vec<(u64, Url)> = playlist
                .segments
                .iter()
                .enumerate()
                .map(|(i, s)| (playlist.media_sequence + i as u64, s))
                .filter(|(seq, _)| !seen.contains(seq))
                .filter_map(|(seq, s)| Some((seq, self.playlist_url.join(&s.uri).ok()?)))
                .collect();

            // `buffered` (not `buffer_unordered`) keeps segment order.
            let mut fetches = futures::stream::iter(fresh)
                .map(|(seq, url)| async move {
                    let bytes = self.pool.send_with_retry(|p| p.get(&url, DeviceClass::Desktop)).await?.bytes().await?;
                    Ok::<_, CoreError>((seq, bytes))
                })
                .buffered(self.parallel_segments.max(1));

            while let Some(item) = tokio::select! {
                () = self.cancel.cancelled() => { writer.flush().await?; return Err(CoreError::Cancelled) },
                item = fetches.next() => item,
            } {
                let (seq, bytes) = item?;
                self.limiter.acquire(bytes.len()).await;
                writer.write_all(&bytes).await?;
                written += bytes.len() as u64;
                self.counter.add(bytes.len() as u64);
                seen.insert(seq);
            }
            writer.flush().await?;

            if playlist.end_list {
                return Ok(written);
            }
            let wait = Duration::from_secs_f32(playlist.target_duration.max(1) as f32);
            tokio::select! {
                () = self.cancel.cancelled() => return Err(CoreError::Cancelled),
                () = tokio::time::sleep(wait) => {}
            }
        }
    }
}

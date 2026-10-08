//! HLS media-playlist downloader (VOD and live).
//!
//! Segments are fetched concurrently but written strictly in order through a
//! single buffered writer, producing one continuous MPEG-TS/fMP4 file that the
//! remux stage turns into MP4/MKV. Live playlists are re-polled every target
//! duration and new media-sequence numbers appended until the stream ends
//! (`#EXT-X-ENDLIST`), the optional duration limit is reached, or the task is
//! cancelled. A live recording starts at the live edge, not at the start of the
//! DVR window the playlist advertises (YouTube lists the last hour).

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
    /// Stop after this much media (by `#EXTINF` durations) and return normally.
    pub max_duration: Option<Duration>,
    /// Media sequence a live recording starts at, shared between the renditions of
    /// one task (video and audio playlists number their segments alike): the first
    /// job to reach the live edge picks it and the others follow, so the
    /// recordings line up.
    pub live_start: Option<&'a parking_lot::Mutex<Option<u64>>>,
}

/// Segments behind the live edge to start a live recording with.
const LIVE_EDGE_SEGMENTS: usize = 3;

/// The segments of one playlist load to fetch: those after `last` (the newest
/// sequence already written), starting at the live edge on the first load of a
/// live playlist, cut off once `budget` seconds of media are covered. Returns
/// `(sequence, uri, duration)` and whether the budget is used up.
///
/// Live recordings only ever move forward: every reload still lists the whole
/// DVR window, and the part before the starting edge must stay skipped.
fn plan_segments(
    playlist: &MediaPlaylist,
    last: Option<u64>,
    mut budget: Option<f32>,
) -> (Vec<(u64, &str, f32)>, bool) {
    let skip = if last.is_none() && !playlist.end_list {
        playlist.segments.len().saturating_sub(LIVE_EDGE_SEGMENTS)
    } else {
        0
    };
    let mut out = Vec::new();
    for (i, s) in playlist.segments.iter().enumerate().skip(skip) {
        let seq = playlist.media_sequence + i as u64;
        if last.is_some_and(|l| seq <= l) {
            continue;
        }
        if let Some(left) = budget.as_mut() {
            if *left <= 0.0 {
                return (out, true);
            }
            *left -= s.duration;
        }
        out.push((seq, s.uri.as_str(), s.duration));
    }
    let exhausted = budget.is_some_and(|b| b <= 0.0);
    (out, exhausted)
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
        let mut last: Option<u64> = None;
        let mut written = 0u64;
        let mut recorded = 0f32;

        loop {
            let playlist = self.load().await?;
            if last.is_none() && !playlist.end_list {
                let edge = playlist.media_sequence + playlist.segments.len().saturating_sub(LIVE_EDGE_SEGMENTS) as u64;
                let start = self.live_start.map_or(edge, |shared| *shared.lock().get_or_insert(edge));
                last = start.checked_sub(1);
            }
            let budget = self.max_duration.map(|d| d.as_secs_f32() - recorded);
            let (planned, done) = plan_segments(&playlist, last, budget);
            recorded += planned.iter().map(|(_, _, d)| d).sum::<f32>();
            let fresh: Vec<(u64, Url)> = planned
                .into_iter()
                .filter_map(|(seq, uri, _)| Some((seq, self.playlist_url.join(uri).ok()?)))
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
                last = Some(seq);
            }
            writer.flush().await?;

            if playlist.end_list || done {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(first_seq: u64, n: usize, end_list: bool) -> MediaPlaylist {
        let mut text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:{first_seq}\n");
        for i in 0..n {
            text.push_str(&format!("#EXTINF:5.0,\nseg{}.ts\n", first_seq + i as u64));
        }
        if end_list {
            text.push_str("#EXT-X-ENDLIST\n");
        }
        m3u8_rs::parse_media_playlist_res(text.as_bytes()).expect("playlist")
    }

    fn seqs(planned: &[(u64, &str, f32)]) -> Vec<u64> {
        planned.iter().map(|p| p.0).collect()
    }

    #[test]
    fn live_recording_starts_at_the_edge() {
        let p = playlist(1000, 720, false);
        let (planned, done) = plan_segments(&p, None, None);
        assert_eq!(seqs(&planned), [1717, 1718, 1719]);
        assert!(!done);
        // Reloads still list the whole DVR window; only newer segments are taken.
        let p = playlist(1002, 720, false);
        let (planned, _) = plan_segments(&p, Some(1719), None);
        assert_eq!(seqs(&planned), [1720, 1721]);
    }

    #[test]
    fn vod_playlists_start_at_the_beginning() {
        let p = playlist(0, 10, true);
        let (planned, done) = plan_segments(&p, None, None);
        assert_eq!(planned.len(), 10);
        assert!(!done);
    }

    #[test]
    fn duration_budget_stops_the_recording() {
        let p = playlist(0, 10, true);
        // 12 s of budget covers three 5 s segments (the third crosses the limit).
        let (planned, done) = plan_segments(&p, None, Some(12.0));
        assert_eq!(seqs(&planned), [0, 1, 2]);
        assert!(done);
        let (planned, done) = plan_segments(&p, Some(4), Some(100.0));
        assert_eq!(seqs(&planned), [5, 6, 7, 8, 9]);
        assert!(!done);
        let (planned, done) = plan_segments(&p, None, Some(0.0));
        assert!(planned.is_empty() && done);
    }
}

//! Format selection: map a [`DownloadProfile`] onto concrete streams.

use crate::error::{CoreError, Result};
use crate::model::{DownloadProfile, MediaInfo, StreamFormat, StreamKind};

/// The streams to fetch for one task. `audio` is `None` when `video` is muxed.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub video: Option<StreamFormat>,
    pub audio: Option<StreamFormat>,
}

impl Selection {
    pub fn needs_remux(&self) -> bool {
        self.video.is_some() && self.audio.is_some()
    }
}

/// Ranking key: resolution, then fps, then codec efficiency, then bitrate.
fn video_rank(f: &StreamFormat) -> (u32, u32, u8, u64) {
    (
        f.height.unwrap_or(0),
        f.fps.map_or(0, |v| v.round() as u32),
        f.video_codec.map_or(0, |c| c.preference()),
        f.bitrate.unwrap_or(0),
    )
}

pub fn best_audio(info: &MediaInfo) -> Option<&StreamFormat> {
    info.formats.iter().filter(|f| f.kind == StreamKind::AudioOnly).max_by_key(|f| f.bitrate.unwrap_or(0))
}

pub fn select(info: &MediaInfo, profile: &DownloadProfile) -> Result<Selection> {
    match profile {
        DownloadProfile::Video { max_resolution, .. } => {
            let cap = max_resolution.height();
            // Compare on the short edge so portrait 9:16 video is not over-filtered.
            let fits = |f: &&StreamFormat| match (f.width, f.height) {
                (Some(w), Some(h)) => w.min(h) <= cap,
                (_, Some(h)) => h <= cap,
                _ => true,
            };
            let best_split = info
                .formats
                .iter()
                .filter(|f| f.kind == StreamKind::VideoOnly)
                .filter(fits)
                .max_by_key(|f| video_rank(f));
            let best_muxed =
                info.formats.iter().filter(|f| f.kind == StreamKind::Muxed).filter(fits).max_by_key(|f| video_rank(f));

            match (best_split, best_muxed, best_audio(info)) {
                (Some(v), m, Some(a)) if m.is_none_or(|m| video_rank(v) > video_rank(m)) => {
                    Ok(Selection { video: Some(v.clone()), audio: Some(a.clone()) })
                }
                (_, Some(m), _) => Ok(Selection { video: Some(m.clone()), audio: None }),
                (Some(v), None, None) => Ok(Selection { video: Some(v.clone()), audio: None }),
                _ => Err(CoreError::FormatUnavailable(format!("no video stream at or below {}p", cap))),
            }
        }
        DownloadProfile::AudioOnly { .. } => {
            if let Some(a) = best_audio(info) {
                return Ok(Selection { video: None, audio: Some(a.clone()) });
            }
            // Extract audio from the smallest muxed stream instead.
            info.formats
                .iter()
                .filter(|f| f.kind == StreamKind::Muxed)
                .min_by_key(|f| f.height.unwrap_or(u32::MAX))
                .map(|m| Selection { video: None, audio: Some(m.clone()) })
                .ok_or_else(|| CoreError::FormatUnavailable("no audio stream".into()))
        }
        DownloadProfile::NoWatermark => info
            .formats
            .iter()
            .filter(|f| f.kind == StreamKind::Muxed)
            .max_by_key(|f| video_rank(f))
            .map(|m| Selection { video: Some(m.clone()), audio: None })
            .ok_or_else(|| CoreError::FormatUnavailable("no clean rendition".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioOutput, Container, ContentKind, Platform, Resolution, Transport, VideoCodec};
    use url::Url;

    fn fmt(id: &str, kind: StreamKind, h: Option<u32>, codec: Option<VideoCodec>, br: u64) -> StreamFormat {
        StreamFormat {
            id: id.into(),
            url: Url::parse(&format!("https://cdn.example/{id}")).expect("url"),
            kind,
            transport: Transport::Http { content_length: None },
            container: "mp4".into(),
            width: h.map(|h| h * 16 / 9),
            height: h,
            fps: Some(60.0),
            video_codec: codec,
            audio_codec: None,
            bitrate: Some(br),
            hdr: false,
            headers: vec![],
        }
    }

    fn info() -> MediaInfo {
        MediaInfo {
            platform: Platform::YouTube,
            id: "x".into(),
            source_url: Url::parse("https://youtu.be/x").expect("url"),
            title: "t".into(),
            author: None,
            thumbnail: None,
            duration_secs: None,
            published_at: None,
            content_kind: ContentKind::Video,
            formats: vec![
                fmt("571", StreamKind::VideoOnly, Some(4320), Some(VideoCodec::Av1), 90_000_000),
                fmt("272", StreamKind::VideoOnly, Some(4320), Some(VideoCodec::Vp9), 60_000_000),
                fmt("313", StreamKind::VideoOnly, Some(2160), Some(VideoCodec::Vp9), 30_000_000),
                fmt("18", StreamKind::Muxed, Some(360), Some(VideoCodec::H264), 500_000),
                fmt("251", StreamKind::AudioOnly, None, None, 160_000),
                fmt("140", StreamKind::AudioOnly, None, None, 128_000),
            ],
            subtitles: vec![],
        }
    }

    #[test]
    fn ultra_8k_prefers_av1_plus_best_audio() {
        let s = select(&info(), &DownloadProfile::ultra_8k()).expect("selected");
        assert_eq!(s.video.as_ref().map(|f| f.id.as_str()), Some("571"));
        assert_eq!(s.audio.as_ref().map(|f| f.id.as_str()), Some("251"));
        assert!(s.needs_remux());
    }

    #[test]
    fn cap_at_4k() {
        let p = DownloadProfile::Video { max_resolution: Resolution::P2160, container: Container::Mp4 };
        let s = select(&info(), &p).expect("selected");
        assert_eq!(s.video.as_ref().map(|f| f.id.as_str()), Some("313"));
    }

    #[test]
    fn audio_only() {
        let s = select(&info(), &DownloadProfile::AudioOnly { format: AudioOutput::Mp3 }).expect("selected");
        assert!(s.video.is_none());
        assert_eq!(s.audio.as_ref().map(|f| f.id.as_str()), Some("251"));
    }
}

//! Domain model shared between extractors, the engine and the UI bridge.
//!
//! Every type here is `Serialize`/`Deserialize` with camelCase fields so it maps
//! 1:1 onto the TypeScript types in `apps/desktop/src/lib/types.ts`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    YouTube,
    TikTok,
    Instagram,
    Twitch,
    Twitter,
    Facebook,
    Generic,
}

impl Platform {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::YouTube => "YouTube",
            Self::TikTok => "TikTok",
            Self::Instagram => "Instagram",
            Self::Twitch => "Twitch",
            Self::Twitter => "X",
            Self::Facebook => "Facebook",
            Self::Generic => "Web",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    Av1,
    Vp9,
    Hevc,
    H264,
    Unknown,
}

impl VideoCodec {
    /// Parse an RFC 6381 codec string (`av01.0.16M.08`, `vp09.00.51.08`, `avc1.640033`).
    pub fn from_codecs_attr(codecs: &str) -> Self {
        let c = codecs.trim().to_ascii_lowercase();
        if c.starts_with("av01") {
            Self::Av1
        } else if c.starts_with("vp09") || c.starts_with("vp9") {
            Self::Vp9
        } else if c.starts_with("hvc1") || c.starts_with("hev1") {
            Self::Hevc
        } else if c.starts_with("avc1") || c.starts_with("avc3") {
            Self::H264
        } else {
            Self::Unknown
        }
    }

    /// Relative efficiency preference used when two formats share a resolution.
    pub const fn preference(self) -> u8 {
        match self {
            Self::Av1 => 4,
            Self::Vp9 => 3,
            Self::Hevc => 2,
            Self::H264 => 1,
            Self::Unknown => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Opus,
    Aac,
    Mp3,
    Flac,
    Unknown,
}

impl AudioCodec {
    pub fn from_codecs_attr(codecs: &str) -> Self {
        let c = codecs.trim().to_ascii_lowercase();
        if c.starts_with("opus") {
            Self::Opus
        } else if c.starts_with("mp3") || c == "mp4a.40.34" || c == "mp4a.6b" {
            Self::Mp3
        } else if c.starts_with("mp4a") {
            Self::Aac
        } else if c.starts_with("flac") {
            Self::Flac
        } else {
            Self::Unknown
        }
    }
}

/// Which elementary streams a format carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamKind {
    /// Progressive file with audio and video muxed together.
    Muxed,
    VideoOnly,
    AudioOnly,
}

/// How the bytes are delivered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "type")]
pub enum Transport {
    /// Single resource; supports HTTP `Range` if `content_length` is known.
    Http { content_length: Option<u64> },
    /// HLS media playlist (VOD or live).
    Hls { live: bool },
    /// DASH representation.
    Dash,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamFormat {
    /// Platform-specific identifier (YouTube itag, HLS variant index, ...).
    pub id: String,
    pub url: Url,
    pub kind: StreamKind,
    pub transport: Transport,
    pub container: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f32>,
    pub video_codec: Option<VideoCodec>,
    pub audio_codec: Option<AudioCodec>,
    /// Average bitrate in bits per second.
    pub bitrate: Option<u64>,
    pub hdr: bool,
    /// Extra request headers the CDN requires (referer, cookies, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
}

impl StreamFormat {
    pub fn has_video(&self) -> bool {
        matches!(self.kind, StreamKind::Muxed | StreamKind::VideoOnly)
    }

    pub fn has_audio(&self) -> bool {
        matches!(self.kind, StreamKind::Muxed | StreamKind::AudioOnly)
    }

    pub fn resolution_label(&self) -> Option<String> {
        self.height.map(|h| Resolution::from_height(h).label().to_owned())
    }
}

/// Named resolution tiers surfaced in the format picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Resolution {
    #[serde(rename = "360p")]
    P360,
    #[serde(rename = "480p")]
    P480,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "1440p")]
    P1440,
    #[serde(rename = "2160p")]
    P2160,
    #[serde(rename = "4320p")]
    P4320,
}

impl Resolution {
    pub const fn height(self) -> u32 {
        match self {
            Self::P360 => 360,
            Self::P480 => 480,
            Self::P720 => 720,
            Self::P1080 => 1080,
            Self::P1440 => 1440,
            Self::P2160 => 2160,
            Self::P4320 => 4320,
        }
    }

    /// Bucket an arbitrary pixel height (portrait videos included) into a tier.
    pub const fn from_height(h: u32) -> Self {
        match h {
            0..=400 => Self::P360,
            401..=600 => Self::P480,
            601..=900 => Self::P720,
            901..=1200 => Self::P1080,
            1201..=1800 => Self::P1440,
            1801..=3000 => Self::P2160,
            _ => Self::P4320,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::P360 => "360p",
            Self::P480 => "480p",
            Self::P720 => "720p HD",
            Self::P1080 => "1080p Full HD",
            Self::P1440 => "1440p QHD",
            Self::P2160 => "2160p 4K",
            Self::P4320 => "4320p 8K",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrack {
    /// BCP-47 language tag (`en`, `pt-BR`).
    pub language: String,
    pub label: String,
    pub url: Url,
    /// `vtt`, `srt`, `ttml`, ...
    pub format: String,
    pub auto_generated: bool,
}

/// What kind of item this is on the platform; drives organizer media separation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentKind {
    Video,
    Short,
    Story,
    LiveStream,
    Vod,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub platform: Platform,
    pub id: String,
    pub source_url: Url,
    pub title: String,
    pub author: Option<String>,
    pub thumbnail: Option<Url>,
    pub duration_secs: Option<f64>,
    pub published_at: Option<DateTime<Utc>>,
    pub content_kind: ContentKind,
    pub formats: Vec<StreamFormat>,
    #[serde(default)]
    pub subtitles: Vec<SubtitleTrack>,
    /// Caveats the user should see, e.g. that only a lower tier than the source
    /// offers could be fetched.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl MediaInfo {
    pub fn max_height(&self) -> Option<u32> {
        self.formats.iter().filter_map(|f| f.height).max()
    }
}

/// Result of running an extractor: a single item or a collection to expand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "type")]
pub enum Extraction {
    Media(Box<MediaInfo>),
    Collection {
        platform: Platform,
        title: String,
        /// Entry URLs to be fed back through the registry.
        entries: Vec<Url>,
        /// Opaque continuation token for paginated collections.
        continuation: Option<String>,
    },
}

/// User-facing download presets shown in the batch format picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "type")]
pub enum DownloadProfile {
    /// Best video up to the given tier plus best audio.
    Video { max_resolution: Resolution, container: Container },
    /// Audio only, transcoded to the given codec.
    AudioOnly { format: AudioOutput },
    /// TikTok/Instagram: the clean (no-overlay) source rendition.
    NoWatermark,
}

impl DownloadProfile {
    pub const fn ultra_8k() -> Self {
        Self::Video { max_resolution: Resolution::P4320, container: Container::Mkv }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    Mp4,
    Mkv,
}

impl Container {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioOutput {
    Mp3,
    M4a,
    Opus,
    Flac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    Queued,
    Scheduled,
    Fetching,
    Downloading,
    Paused,
    Muxing,
    Organizing,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Scheduled => "scheduled",
            Self::Fetching => "fetching",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Muxing => "muxing",
            Self::Organizing => "organizing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "scheduled" => Self::Scheduled,
            "fetching" => Self::Fetching,
            "downloading" => Self::Downloading,
            "paused" => Self::Paused,
            "muxing" => Self::Muxing,
            "organizing" => Self::Organizing,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// A queued download as persisted in SQLite and shown in the queue table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub id: Uuid,
    pub source_url: Url,
    pub platform: Platform,
    pub title: String,
    pub author: Option<String>,
    pub profile: DownloadProfile,
    pub state: TaskState,
    pub bytes_total: Option<u64>,
    pub bytes_done: u64,
    pub destination: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_detection() {
        assert_eq!(VideoCodec::from_codecs_attr("av01.0.16M.08"), VideoCodec::Av1);
        assert_eq!(VideoCodec::from_codecs_attr("vp09.00.51.08"), VideoCodec::Vp9);
        assert_eq!(VideoCodec::from_codecs_attr("avc1.640033"), VideoCodec::H264);
        assert_eq!(AudioCodec::from_codecs_attr("mp4a.40.2"), AudioCodec::Aac);
        assert_eq!(AudioCodec::from_codecs_attr("opus"), AudioCodec::Opus);
    }

    #[test]
    fn resolution_buckets_cover_8k() {
        assert_eq!(Resolution::from_height(4320), Resolution::P4320);
        assert_eq!(Resolution::from_height(2160), Resolution::P2160);
        assert_eq!(Resolution::from_height(1920), Resolution::P2160);
        assert_eq!(Resolution::from_height(1080), Resolution::P1080);
    }

    #[test]
    fn wire_format_matches_typescript_types() {
        let p = serde_json::to_value(DownloadProfile::ultra_8k()).expect("json");
        assert_eq!(p, serde_json::json!({ "type": "video", "maxResolution": "4320p", "container": "mkv" }));
        let t = serde_json::to_value(Transport::Http { content_length: Some(1) }).expect("json");
        assert_eq!(t, serde_json::json!({ "type": "http", "contentLength": 1 }));
        assert_eq!(serde_json::to_value(Platform::YouTube).expect("json"), "youtube");
        assert_eq!(serde_json::to_value(ContentKind::LiveStream).expect("json"), "liveStream");
    }

    #[test]
    fn task_state_roundtrip() {
        for s in [TaskState::Queued, TaskState::Muxing, TaskState::Completed] {
            assert_eq!(TaskState::parse(s.as_str()), Some(s));
        }
    }
}

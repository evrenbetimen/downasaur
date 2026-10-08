//! Universal fallback: bridge to `yt-dlp`.
//!
//! For any host without a native module, Downasaur shells out to
//! `yt-dlp --dump-single-json` and maps its info dict onto [`MediaInfo`]. This
//! gives coverage of thousands of sites while the native modules keep the hot
//! paths fast and dependency-free. Only metadata comes from yt-dlp; the native
//! downloader still fetches the bytes so pause/resume and progress stay uniform.

use std::process::Stdio;

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;
use url::Url;

use super::util::{at, at_str, at_u64};
use super::{ExtractContext, PlatformExtractor};
use crate::error::{CoreError, DrmScheme, Result};
use crate::model::{
    AudioCodec, ContentKind, Extraction, MediaInfo, Platform, StreamFormat, StreamKind, SubtitleTrack, Transport,
    VideoCodec,
};

const NAME: &str = "yt-dlp";

#[derive(Debug, Default, Clone, Copy)]
pub struct YtDlpExtractor;

#[async_trait]
impl PlatformExtractor for YtDlpExtractor {
    fn platform(&self) -> Platform {
        Platform::Generic
    }

    fn matches(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
    }

    async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
        let output = Command::new(&ctx.yt_dlp)
            .args(["--dump-single-json", "--no-warnings", "--no-playlist", "--flat-playlist", "--"])
            .arg(url.as_str())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => CoreError::ToolMissing("yt-dlp"),
                _ => CoreError::Io(e),
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("DRM") {
                return Err(CoreError::DrmProtected(DrmScheme::Unknown));
            }
            return Err(CoreError::extraction(NAME, stderr.lines().last().unwrap_or("unknown error").to_owned()));
        }
        let info: Value =
            serde_json::from_slice(&output.stdout).map_err(|e| CoreError::Parse(format!("yt-dlp JSON: {e}")))?;
        parse_info_dict(&info, url)
    }
}

pub fn parse_info_dict(info: &Value, source: &Url) -> Result<Extraction> {
    if at_str(info, "_type") == Some("playlist") {
        let entries = at(info, "entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| at_str(e, "url").or_else(|| at_str(e, "webpage_url")))
            .filter_map(|u| Url::parse(u).ok())
            .collect();
        return Ok(Extraction::Collection {
            platform: Platform::Generic,
            title: at_str(info, "title").unwrap_or("Playlist").to_owned(),
            entries,
            continuation: None,
        });
    }

    let raw_formats = at(info, "formats").and_then(Value::as_array).cloned().unwrap_or_default();
    if raw_formats.iter().any(|f| f.get("has_drm").and_then(Value::as_bool) == Some(true))
        && raw_formats.iter().all(|f| f.get("has_drm").and_then(Value::as_bool) == Some(true))
    {
        return Err(CoreError::DrmProtected(DrmScheme::Unknown));
    }

    let formats = raw_formats.iter().filter_map(map_format).collect::<Vec<_>>();
    if formats.is_empty() {
        return Err(CoreError::extraction(NAME, "no downloadable formats"));
    }

    let subtitles = at(info, "subtitles")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(lang, tracks)| {
            let t = tracks.as_array()?.iter().find(|t| matches!(at_str(t, "ext"), Some("vtt" | "srt")))?;
            Some(SubtitleTrack {
                language: lang.clone(),
                label: at_str(t, "name").unwrap_or(lang).to_owned(),
                url: Url::parse(at_str(t, "url")?).ok()?,
                format: at_str(t, "ext")?.to_owned(),
                auto_generated: false,
            })
        })
        .collect();

    Ok(Extraction::Media(Box::new(MediaInfo {
        platform: Platform::Generic,
        id: at_str(info, "id").unwrap_or_default().to_owned(),
        source_url: source.clone(),
        title: at_str(info, "title").unwrap_or("Video").to_owned(),
        author: at_str(info, "uploader").or_else(|| at_str(info, "channel")).map(str::to_owned),
        thumbnail: at_str(info, "thumbnail").and_then(|u| Url::parse(u).ok()),
        duration_secs: at(info, "duration").and_then(Value::as_f64),
        published_at: at_u64(info, "timestamp")
            .and_then(|t| chrono::DateTime::from_timestamp(i64::try_from(t).ok()?, 0)),
        content_kind: if info.get("is_live").and_then(Value::as_bool) == Some(true) {
            ContentKind::LiveStream
        } else {
            ContentKind::Video
        },
        formats,
        subtitles,
        warnings: Vec::new(),
    })))
}

fn map_format(f: &Value) -> Option<StreamFormat> {
    if f.get("has_drm").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let url = Url::parse(at_str(f, "url")?).ok()?;
    let vcodec = at_str(f, "vcodec").filter(|c| *c != "none");
    let acodec = at_str(f, "acodec").filter(|c| *c != "none");
    let kind = match (vcodec, acodec) {
        (Some(_), Some(_)) => StreamKind::Muxed,
        (Some(_), None) => StreamKind::VideoOnly,
        (None, Some(_)) => StreamKind::AudioOnly,
        // yt-dlp leaves codecs unset for many progressive sources.
        (None, None) => StreamKind::Muxed,
    };
    let protocol = at_str(f, "protocol").unwrap_or("https");
    let transport = if protocol.starts_with("m3u8") {
        Transport::Hls { live: false }
    } else if protocol.contains("dash") {
        Transport::Dash
    } else {
        Transport::Http { content_length: at_u64(f, "filesize") }
    };
    let headers = at(f, "http_headers")
        .and_then(Value::as_object)
        .map(|h| h.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        .unwrap_or_default();

    Some(StreamFormat {
        id: at_str(f, "format_id").unwrap_or("0").to_owned(),
        url,
        kind,
        transport,
        container: at_str(f, "ext").unwrap_or("mp4").to_owned(),
        width: at_u64(f, "width").and_then(|v| u32::try_from(v).ok()),
        height: at_u64(f, "height").and_then(|v| u32::try_from(v).ok()),
        fps: at(f, "fps").and_then(Value::as_f64).map(|v| v as f32),
        video_codec: vcodec.map(VideoCodec::from_codecs_attr),
        audio_codec: acodec.map(AudioCodec::from_codecs_attr),
        bitrate: at(f, "tbr").and_then(Value::as_f64).map(|k| (k * 1000.0) as u64),
        hdr: at_str(f, "dynamic_range").is_some_and(|d| d != "SDR"),
        headers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_info_dict() {
        let info = json!({
            "id": "76979871", "title": "The New Vimeo Player", "uploader": "Vimeo", "duration": 62.0,
            "formats": [
                { "format_id": "hls-1080p", "url": "https://vod.example/1080.m3u8", "protocol": "m3u8_native",
                  "ext": "mp4", "width": 1920, "height": 1080, "vcodec": "avc1.640028", "acodec": "mp4a.40.2", "tbr": 4500.0 },
                { "format_id": "drm", "url": "https://vod.example/drm.mpd", "has_drm": true }
            ]
        });
        let src = Url::parse("https://vimeo.com/76979871").expect("url");
        let Extraction::Media(m) = parse_info_dict(&info, &src).expect("parsed") else { panic!("expected media") };
        assert_eq!(m.formats.len(), 1);
        assert_eq!(m.formats[0].transport, Transport::Hls { live: false });
        assert_eq!(m.formats[0].bitrate, Some(4_500_000));
    }

    #[test]
    fn all_drm_formats_raise_drm_error() {
        let info = json!({ "id": "x", "formats": [ { "format_id": "a", "url": "https://x/a.mpd", "has_drm": true } ] });
        let src = Url::parse("https://example.com/v").expect("url");
        assert_eq!(parse_info_dict(&info, &src).expect_err("drm").code(), "drm_protected");
    }
}

//! YouTube: videos, Shorts, live streams, playlists and channels.
//!
//! Adaptive formats go up to 4320p60 (AV1 itag 571/402, VP9 itag 272/337). Video and
//! audio arrive as separate DASH-style streams and are joined by the remux stage.
//!
//! Sub-modules:
//! - [`urls`]: URL classification (watch / shorts / live / playlist / channel).
//! - [`formats`]: `streamingData` → [`StreamFormat`] mapping and best-format selection.
//! - [`cipher`]: `signatureCipher` unpacking and transform-program evaluation.
//! - [`playlist`]: playlist/channel crawling with continuation tokens.

pub mod cipher;
pub mod formats;
pub mod playlist;
pub mod urls;

use async_trait::async_trait;
use serde_json::{Value, json};
use url::Url;

use super::util::{at, at_str, at_u64};
use super::{ExtractContext, PlatformExtractor, host_is};
use crate::error::{CoreError, DrmScheme, Result};
use crate::model::{ContentKind, Extraction, MediaInfo, Platform, SubtitleTrack};
use crate::net::fingerprint::DeviceClass;
use urls::YouTubeUrl;

const NAME: &str = "YouTube";
const PLAYER_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";

#[derive(Debug, Default, Clone, Copy)]
pub struct YouTubeExtractor;

#[async_trait]
impl PlatformExtractor for YouTubeExtractor {
    fn platform(&self) -> Platform {
        Platform::YouTube
    }

    fn matches(&self, url: &Url) -> bool {
        ["youtube.com", "youtu.be", "youtube-nocookie.com"].iter().any(|d| host_is(url, d))
    }

    async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
        match urls::classify(url) {
            Some(YouTubeUrl::Video { id, kind }) => {
                let player = fetch_player_response(ctx, &id).await?;
                let mut info = parse_player_response(&player, url, kind)?;
                formats::resolve_ciphers(ctx, &mut info, &player).await?;
                Ok(Extraction::Media(Box::new(info)))
            }
            Some(YouTubeUrl::Playlist { list }) => playlist::crawl_playlist(ctx, &list).await,
            Some(YouTubeUrl::Channel { path }) => playlist::crawl_channel(ctx, &path).await,
            None => Err(CoreError::UnsupportedUrl(url.to_string())),
        }
    }
}

/// Query the InnerTube player endpoint for a video's `playerResponse`.
async fn fetch_player_response(ctx: &ExtractContext, video_id: &str) -> Result<Value> {
    let endpoint = Url::parse(PLAYER_ENDPOINT)?;
    let body = json!({
        "videoId": video_id,
        "context": { "client": { "clientName": "WEB", "clientVersion": "2.20260901.00.00", "hl": "en" } },
        "playbackContext": { "contentPlaybackContext": { "html5Preference": "HTML5_PREF_WANTS" } },
        "contentCheckOk": true,
        "racyCheckOk": true
    });
    let resp = ctx
        .http
        .send_with_retry(|pool| {
            pool.get(&endpoint, DeviceClass::Desktop).json(&body).header("Origin", "https://www.youtube.com")
        })
        .await?;
    Ok(resp.json().await?)
}

/// Turn an InnerTube `playerResponse` into [`MediaInfo`].
///
/// Formats that still carry a `signatureCipher` keep their raw base URL here;
/// [`formats::resolve_ciphers`] rewrites them once the player program is known.
pub fn parse_player_response(player: &Value, source: &Url, kind: ContentKind) -> Result<MediaInfo> {
    match at_str(player, "playabilityStatus/status") {
        Some("OK") | None => {}
        Some(status) => {
            let reason = at_str(player, "playabilityStatus/reason").unwrap_or(status);
            return Err(CoreError::extraction(NAME, reason.to_owned()));
        }
    }

    // Purchased/rental titles advertise license servers or DRM families.
    let drm = at(player, "streamingData/licenseInfos").is_some()
        || at(player, "streamingData/adaptiveFormats")
            .and_then(Value::as_array)
            .is_some_and(|fs| fs.iter().any(|f| f.get("drmFamilies").is_some()));
    if drm {
        return Err(CoreError::DrmProtected(DrmScheme::Widevine));
    }

    let details = at(player, "videoDetails").ok_or_else(|| super::util::parse_err(NAME, "videoDetails"))?;
    let id = at_str(details, "videoId").unwrap_or_default().to_owned();
    let is_live = details.get("isLive").and_then(Value::as_bool).unwrap_or(false);

    let thumbnail = at(details, "thumbnail/thumbnails")
        .and_then(Value::as_array)
        .and_then(|t| t.iter().max_by_key(|x| at_u64(x, "width").unwrap_or(0)))
        .and_then(|t| at_str(t, "url"))
        .and_then(|u| Url::parse(u).ok());

    let published_at = at_str(player, "microformat/playerMicroformatRenderer/publishDate")
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
        .map(|d| d.with_timezone(&chrono::Utc));

    Ok(MediaInfo {
        platform: Platform::YouTube,
        id,
        source_url: source.clone(),
        title: at_str(details, "title").unwrap_or("Untitled").to_owned(),
        author: at_str(details, "author").map(str::to_owned),
        thumbnail,
        duration_secs: at_u64(details, "lengthSeconds").map(|s| s as f64),
        published_at,
        content_kind: if is_live { ContentKind::LiveStream } else { kind },
        formats: formats::parse_streaming_data(player),
        subtitles: parse_captions(player),
    })
}

fn parse_captions(player: &Value) -> Vec<SubtitleTrack> {
    at(player, "captions/playerCaptionsTracklistRenderer/captionTracks")
        .and_then(Value::as_array)
        .map(|tracks| {
            tracks
                .iter()
                .filter_map(|t| {
                    let mut url = Url::parse(at_str(t, "baseUrl")?).ok()?;
                    url.query_pairs_mut().append_pair("fmt", "vtt");
                    Some(SubtitleTrack {
                        language: at_str(t, "languageCode")?.to_owned(),
                        label: at_str(t, "name/simpleText")
                            .or_else(|| at_str(t, "name/runs/0/text"))
                            .unwrap_or_default()
                            .to_owned(),
                        url,
                        format: "vtt".into(),
                        auto_generated: at_str(t, "kind") == Some("asr"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_player() -> Value {
        json!({
            "playabilityStatus": { "status": "OK" },
            "videoDetails": {
                "videoId": "abcdefghijk", "title": "8K Demo", "author": "Creator",
                "lengthSeconds": "300", "isLive": false,
                "thumbnail": { "thumbnails": [ { "url": "https://i.ytimg.com/vi/a/hq.jpg", "width": 480 },
                                               { "url": "https://i.ytimg.com/vi/a/max.jpg", "width": 1280 } ] }
            },
            "streamingData": { "adaptiveFormats": [
                { "itag": 571, "url": "https://rr.googlevideo.com/a?itag=571", "mimeType": "video/mp4; codecs=\"av01.0.16M.08\"",
                  "width": 7680, "height": 4320, "fps": 60, "bitrate": 90000000, "contentLength": "3000000000" },
                { "itag": 272, "url": "https://rr.googlevideo.com/a?itag=272", "mimeType": "video/webm; codecs=\"vp9\"",
                  "width": 7680, "height": 4320, "fps": 30, "bitrate": 60000000 },
                { "itag": 251, "url": "https://rr.googlevideo.com/a?itag=251", "mimeType": "audio/webm; codecs=\"opus\"",
                  "bitrate": 160000 }
            ]},
            "captions": { "playerCaptionsTracklistRenderer": { "captionTracks": [
                { "baseUrl": "https://www.youtube.com/api/timedtext?v=a&lang=en", "languageCode": "en",
                  "name": { "simpleText": "English (auto-generated)" }, "kind": "asr" }
            ]}}
        })
    }

    #[test]
    fn parses_8k_player_response() {
        let src = Url::parse("https://youtu.be/abcdefghijk").expect("url");
        let info = parse_player_response(&sample_player(), &src, ContentKind::Video).expect("parsed");
        assert_eq!(info.title, "8K Demo");
        assert_eq!(info.max_height(), Some(4320));
        assert_eq!(info.formats.len(), 3);
        assert_eq!(info.thumbnail.as_ref().map(Url::as_str), Some("https://i.ytimg.com/vi/a/max.jpg"));
        assert!(info.subtitles[0].auto_generated);
    }

    #[test]
    fn drm_titles_are_rejected_gracefully() {
        let mut p = sample_player();
        p["streamingData"]["licenseInfos"] = json!([{ "drmFamily": "WIDEVINE" }]);
        let src = Url::parse("https://youtu.be/abcdefghijk").expect("url");
        let err = parse_player_response(&p, &src, ContentKind::Video).expect_err("drm");
        assert_eq!(err.code(), "drm_protected");
    }
}

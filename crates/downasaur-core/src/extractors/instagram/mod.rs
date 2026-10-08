//! Instagram posts, Reels and Stories.
//!
//! Public posts and Reels expose `video_versions` (or `video_url` on older
//! payloads) inside the embedded `xdt_api__v1__media__shortcode__web_info`
//! response. Stories are only visible to signed-in users, so they require the
//! user to import their own session cookie.

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use super::util::{at, at_str, at_u64, json_string_field};
use super::{ExtractContext, PlatformExtractor, host_is};
use crate::error::{CoreError, Result};
use crate::model::{ContentKind, Extraction, MediaInfo, Platform, StreamFormat, StreamKind, Transport, VideoCodec};
use crate::net::fingerprint::DeviceClass;

const NAME: &str = "Instagram";

#[derive(Debug, Default, Clone, Copy)]
pub struct InstagramExtractor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstagramUrl {
    Post { shortcode: String, reel: bool },
    Story { user: String, media_id: String },
}

pub fn classify(url: &Url) -> Option<InstagramUrl> {
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        ["p" | "tv", code, ..] => Some(InstagramUrl::Post { shortcode: (*code).to_owned(), reel: false }),
        ["reel" | "reels", code, ..] => Some(InstagramUrl::Post { shortcode: (*code).to_owned(), reel: true }),
        ["stories", user, id, ..] => Some(InstagramUrl::Story { user: (*user).to_owned(), media_id: (*id).to_owned() }),
        _ => None,
    }
}

#[async_trait]
impl PlatformExtractor for InstagramExtractor {
    fn platform(&self) -> Platform {
        Platform::Instagram
    }

    fn matches(&self, url: &Url) -> bool {
        host_is(url, "instagram.com") || host_is(url, "instagr.am")
    }

    async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
        let target = classify(url).ok_or_else(|| CoreError::UnsupportedUrl(url.to_string()))?;
        let cookie = ctx.cookie_for(Platform::Instagram);
        if matches!(target, InstagramUrl::Story { .. }) && cookie.is_none() {
            return Err(CoreError::extraction(
                NAME,
                "Stories require signing in: import your Instagram session in Settings.",
            ));
        }

        let html = {
            let resp = ctx
                .http
                .send_with_retry(|pool| {
                    let req = pool.get(url, DeviceClass::Mobile).header("X-IG-App-ID", "936619743392459");
                    match cookie {
                        Some(c) => req.header("Cookie", c),
                        None => req,
                    }
                })
                .await?;
            resp.text().await?
        };

        let kind = match target {
            InstagramUrl::Post { reel: true, .. } => ContentKind::Short,
            InstagramUrl::Post { .. } => ContentKind::Video,
            InstagramUrl::Story { .. } => ContentKind::Story,
        };

        if let Some(item) = find_media_item(&html) {
            return Ok(Extraction::Media(Box::new(parse_media_item(&item, url, kind)?)));
        }
        // Minimal fallback: OpenGraph video tag.
        let og = json_string_field(&html, "video_url")
            .ok_or_else(|| CoreError::extraction(NAME, "no video found (the post may be private or a photo)"))?;
        let item = serde_json::json!({ "video_versions": [{ "url": og }] });
        Ok(Extraction::Media(Box::new(parse_media_item(&item, url, kind)?)))
    }
}

/// Locate the first media item JSON object containing `video_versions`.
fn find_media_item(html: &str) -> Option<Value> {
    let key = "\"xdt_api__v1__media__shortcode__web_info\":";
    let start = html.find(key)? + key.len();
    let mut de = serde_json::Deserializer::from_str(&html[start..]).into_iter::<Value>();
    let v = de.next()?.ok()?;
    at(&v, "items/0").cloned()
}

pub fn parse_media_item(item: &Value, source: &Url, kind: ContentKind) -> Result<MediaInfo> {
    let formats: Vec<StreamFormat> = at(item, "video_versions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, v)| {
            Some(StreamFormat {
                id: at_str(v, "id").map_or_else(|| i.to_string(), str::to_owned),
                url: Url::parse(at_str(v, "url")?).ok()?,
                kind: StreamKind::Muxed,
                transport: Transport::Http { content_length: None },
                container: "mp4".into(),
                width: at_u64(v, "width").and_then(|x| u32::try_from(x).ok()),
                height: at_u64(v, "height").and_then(|x| u32::try_from(x).ok()),
                fps: None,
                video_codec: Some(VideoCodec::H264),
                audio_codec: None,
                bitrate: None,
                hdr: false,
                headers: Vec::new(),
            })
        })
        .collect();
    if formats.is_empty() {
        return Err(CoreError::extraction(NAME, "post has no video versions"));
    }

    Ok(MediaInfo {
        platform: Platform::Instagram,
        id: at_str(item, "code").or_else(|| at_str(item, "id")).unwrap_or_default().to_owned(),
        source_url: source.clone(),
        title: at_str(item, "caption/text")
            .map_or_else(|| "Instagram video".to_owned(), |c| c.lines().next().unwrap_or(c).to_owned()),
        author: at_str(item, "user/username").map(str::to_owned),
        thumbnail: at_str(item, "image_versions2/candidates/0/url").and_then(|u| Url::parse(u).ok()),
        duration_secs: at(item, "video_duration").and_then(Value::as_f64),
        published_at: at_u64(item, "taken_at")
            .and_then(|t| chrono::DateTime::from_timestamp(i64::try_from(t).ok()?, 0)),
        content_kind: kind,
        formats,
        subtitles: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_reels_and_stories() {
        let u = |s: &str| Url::parse(s).expect("url");
        assert_eq!(
            classify(&u("https://www.instagram.com/reel/Cxyz123/")),
            Some(InstagramUrl::Post { shortcode: "Cxyz123".into(), reel: true })
        );
        assert_eq!(
            classify(&u("https://www.instagram.com/stories/someone/3100000000/")),
            Some(InstagramUrl::Story { user: "someone".into(), media_id: "3100000000".into() })
        );
    }

    #[test]
    fn parses_video_versions() {
        let item = json!({
            "code": "Cxyz123", "taken_at": 1700000000, "video_duration": 12.5,
            "user": { "username": "creator" }, "caption": { "text": "Sunset reel\nmore text" },
            "video_versions": [ { "url": "https://scontent.cdninstagram.com/v.mp4", "width": 1080, "height": 1920 } ]
        });
        let src = Url::parse("https://www.instagram.com/reel/Cxyz123/").expect("url");
        let info = parse_media_item(&item, &src, ContentKind::Short).expect("parsed");
        assert_eq!(info.title, "Sunset reel");
        assert_eq!(info.formats[0].height, Some(1920));
    }
}

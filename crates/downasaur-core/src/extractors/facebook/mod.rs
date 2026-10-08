//! Facebook videos, Watch and Reels.
//!
//! The page HTML carries the video's GraphQL payload inline. HD/SD progressive
//! URLs appear under `browser_native_hd_url` / `playable_url_quality_hd` (newer
//! and older payloads respectively), and JSON-LD `VideoObject` blocks provide
//! title, author and upload date.

use async_trait::async_trait;
use url::Url;

use super::util::{at_str, json_ld_blocks, json_string_field};
use super::{ExtractContext, PlatformExtractor, host_is};
use crate::error::{CoreError, Result};
use crate::model::{ContentKind, Extraction, MediaInfo, Platform, StreamFormat, StreamKind, Transport, VideoCodec};
use crate::net::fingerprint::DeviceClass;

const NAME: &str = "Facebook";

/// Ordered best → worst.
const URL_KEYS: &[(&str, &str)] = &[
    ("browser_native_hd_url", "hd"),
    ("playable_url_quality_hd", "hd"),
    ("hd_src", "hd"),
    ("browser_native_sd_url", "sd"),
    ("playable_url", "sd"),
    ("sd_src", "sd"),
];

#[derive(Debug, Default, Clone, Copy)]
pub struct FacebookExtractor;

#[async_trait]
impl PlatformExtractor for FacebookExtractor {
    fn platform(&self) -> Platform {
        Platform::Facebook
    }

    fn matches(&self, url: &Url) -> bool {
        host_is(url, "facebook.com") || host_is(url, "fb.watch")
    }

    async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
        let html = ctx
            .http
            .send_with_retry(|pool| {
                let req = pool.get(url, DeviceClass::Desktop).header("Sec-Fetch-Mode", "navigate");
                match ctx.cookie_for(Platform::Facebook) {
                    Some(c) => req.header("Cookie", c),
                    None => req,
                }
            })
            .await?
            .text()
            .await?;
        Ok(Extraction::Media(Box::new(parse_page(&html, url)?)))
    }
}

pub fn parse_page(html: &str, source: &Url) -> Result<MediaInfo> {
    let mut seen = Vec::<String>::new();
    let formats: Vec<StreamFormat> = URL_KEYS
        .iter()
        .filter_map(|(key, label)| {
            let raw = json_string_field(html, key)?;
            if seen.contains(&raw) {
                return None;
            }
            seen.push(raw.clone());
            Some(StreamFormat {
                id: (*label).to_owned(),
                url: Url::parse(&raw).ok()?,
                kind: StreamKind::Muxed,
                transport: Transport::Http { content_length: None },
                container: "mp4".into(),
                width: None,
                height: None,
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
        return Err(CoreError::extraction(NAME, "no playable video URL (the video may be private)"));
    }

    let ld = json_ld_blocks(html).into_iter().find(|v| at_str(v, "@type") == Some("VideoObject"));
    let ld_str = |k: &str| ld.as_ref().and_then(|v| at_str(v, k)).map(str::to_owned);
    let is_reel = source.path().contains("/reel");

    Ok(MediaInfo {
        platform: Platform::Facebook,
        id: json_string_field(html, "video_id").unwrap_or_default(),
        source_url: source.clone(),
        title: ld_str("name").unwrap_or_else(|| "Facebook video".into()),
        author: ld.as_ref().and_then(|v| at_str(v, "author/name")).map(str::to_owned),
        thumbnail: ld_str("thumbnailUrl").and_then(|u| Url::parse(&u).ok()),
        duration_secs: None,
        published_at: ld_str("uploadDate")
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(&d).ok())
            .map(|d| d.with_timezone(&chrono::Utc)),
        content_kind: if is_reel { ContentKind::Short } else { ContentKind::Video },
        formats,
        subtitles: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hd_and_sd() {
        let html = r#"<html><script type="application/ld+json">{"@type":"VideoObject","name":"Trip","author":{"name":"Ana"}}</script>
            <script>{"video_id":"123","browser_native_hd_url":"https:\/\/video.xx.fbcdn.net\/hd.mp4","browser_native_sd_url":"https:\/\/video.xx.fbcdn.net\/sd.mp4","playable_url_quality_hd":"https:\/\/video.xx.fbcdn.net\/hd.mp4"}</script></html>"#;
        let src = Url::parse("https://www.facebook.com/watch/?v=123").expect("url");
        let info = parse_page(html, &src).expect("parsed");
        assert_eq!(info.formats.len(), 2);
        assert_eq!(info.formats[0].id, "hd");
        assert_eq!(info.title, "Trip");
        assert_eq!(info.author.as_deref(), Some("Ana"));
    }
}

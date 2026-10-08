//! TikTok videos.
//!
//! The web page embeds `__UNIVERSAL_DATA_FOR_REHYDRATION__`, whose
//! `itemStruct.video` lists `bitrateInfo[].PlayAddr` renditions. Those play
//! renditions are the clean source encodes; `downloadAddr` is the share copy with
//! the overlay burned in, so it is only used as a last resort. Requests use a
//! mobile browser profile, the cookie jar carries `tt_chain_token`, and CDN
//! fetches must send the TikTok referer.

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use super::util::{at, at_str, at_u64, require, script_json_by_id};
use super::{ExtractContext, PlatformExtractor, host_is};
use crate::error::Result;
use crate::model::{ContentKind, Extraction, MediaInfo, Platform, StreamFormat, StreamKind, Transport, VideoCodec};
use crate::net::fingerprint::DeviceClass;

const NAME: &str = "TikTok";
const REFERER: &str = "https://www.tiktok.com/";

#[derive(Debug, Default, Clone, Copy)]
pub struct TikTokExtractor;

#[async_trait]
impl PlatformExtractor for TikTokExtractor {
    fn platform(&self) -> Platform {
        Platform::TikTok
    }

    fn matches(&self, url: &Url) -> bool {
        host_is(url, "tiktok.com")
    }

    async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
        // Short links (vm./vt.tiktok.com) redirect to the canonical page; reqwest
        // follows redirects, and `resp.url()` gives the final location.
        let resp = ctx
            .http
            .send_with_retry(|pool| {
                let req = pool.get(url, DeviceClass::Mobile).header("Referer", REFERER);
                match ctx.cookie_for(Platform::TikTok) {
                    Some(c) => req.header("Cookie", c),
                    None => req,
                }
            })
            .await?;
        let canonical = resp.url().clone();
        let html = resp.text().await?;
        let data = require(script_json_by_id(&html, "__UNIVERSAL_DATA_FOR_REHYDRATION__"), NAME, "rehydration data")?;
        let item = require(at(&data, "__DEFAULT_SCOPE__/webapp.video-detail/itemInfo/itemStruct"), NAME, "itemStruct")?;
        Ok(Extraction::Media(Box::new(parse_item(item, &canonical)?)))
    }
}

pub fn parse_item(item: &Value, source: &Url) -> Result<MediaInfo> {
    let video = require(item.get("video"), NAME, "video object")?;
    let headers = vec![("Referer".to_owned(), REFERER.to_owned())];

    let mut formats: Vec<StreamFormat> = at(video, "bitrateInfo")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let url = Url::parse(at_str(b, "PlayAddr/UrlList/0")?).ok()?;
            let codec = at_str(b, "CodecType").unwrap_or_default();
            Some(StreamFormat {
                id: at_str(b, "GearName").unwrap_or("play").to_owned(),
                url,
                kind: StreamKind::Muxed,
                transport: Transport::Http { content_length: at_u64(b, "PlayAddr/DataSize") },
                container: "mp4".into(),
                width: at_u64(b, "PlayAddr/Width").and_then(|v| u32::try_from(v).ok()),
                height: at_u64(b, "PlayAddr/Height").and_then(|v| u32::try_from(v).ok()),
                fps: None,
                video_codec: Some(if codec.contains("265") { VideoCodec::Hevc } else { VideoCodec::H264 }),
                audio_codec: None,
                bitrate: at_u64(b, "Bitrate"),
                hdr: false,
                headers: headers.clone(),
            })
        })
        .collect();

    if formats.is_empty() {
        // Fallback: single play address (still overlay-free), then the share copy.
        let fallback = at_str(video, "playAddr").or_else(|| at_str(video, "downloadAddr"));
        let url = Url::parse(require(fallback, NAME, "play address")?)?;
        formats.push(StreamFormat {
            id: "play".into(),
            url,
            kind: StreamKind::Muxed,
            transport: Transport::Http { content_length: None },
            container: "mp4".into(),
            width: at_u64(video, "width").and_then(|v| u32::try_from(v).ok()),
            height: at_u64(video, "height").and_then(|v| u32::try_from(v).ok()),
            fps: None,
            video_codec: Some(VideoCodec::H264),
            audio_codec: None,
            bitrate: at_u64(video, "bitrate"),
            hdr: false,
            headers,
        });
    }

    Ok(MediaInfo {
        platform: Platform::TikTok,
        id: at_str(item, "id").unwrap_or_default().to_owned(),
        source_url: source.clone(),
        title: at_str(item, "desc").filter(|d| !d.is_empty()).unwrap_or("TikTok video").to_owned(),
        author: at_str(item, "author/uniqueId").map(str::to_owned),
        thumbnail: at_str(video, "cover").and_then(|u| Url::parse(u).ok()),
        duration_secs: at_u64(video, "duration").map(|d| d as f64),
        published_at: at_u64(item, "createTime")
            .and_then(|t| chrono::DateTime::from_timestamp(i64::try_from(t).ok()?, 0)),
        content_kind: ContentKind::Short,
        formats,
        subtitles: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefers_clean_play_renditions() {
        let item = json!({
            "id": "7300000000000000000", "desc": "dance #fyp", "createTime": 1700000000,
            "author": { "uniqueId": "creator" },
            "video": {
                "duration": 15, "cover": "https://p16.tiktokcdn.com/c.jpg",
                "downloadAddr": "https://v16.tiktokcdn.com/watermarked.mp4",
                "bitrateInfo": [
                    { "GearName": "normal_1080_0", "Bitrate": 2500000, "CodecType": "h265_hvc1",
                      "PlayAddr": { "UrlList": ["https://v16.tiktokcdn.com/clean1080.mp4"], "Width": 1080, "Height": 1920, "DataSize": 4000000 } }
                ]
            }
        });
        let src = Url::parse("https://www.tiktok.com/@creator/video/7300000000000000000").expect("url");
        let info = parse_item(&item, &src).expect("parsed");
        assert_eq!(info.formats.len(), 1);
        assert!(info.formats[0].url.as_str().contains("clean1080"));
        assert_eq!(info.formats[0].video_codec, Some(VideoCodec::Hevc));
        assert_eq!(info.author.as_deref(), Some("creator"));
    }
}

//! X (Twitter) posts.
//!
//! Media lives in `extended_entities.media[].video_info.variants`; MP4 variants
//! carry a `bitrate` and the highest one is the best quality. The same structure
//! appears in the GraphQL `TweetResultByRestId` payload and the syndication
//! endpoint used here, which needs no login for public posts.

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use super::util::{at, at_str, at_u64};
use super::{ExtractContext, PlatformExtractor, host_is};
use crate::error::{CoreError, Result};
use crate::model::{ContentKind, Extraction, MediaInfo, Platform, StreamFormat, StreamKind, Transport, VideoCodec};
use crate::net::fingerprint::DeviceClass;

const NAME: &str = "X";

pub fn status_id(url: &Url) -> Option<String> {
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let pos = segs.iter().position(|s| *s == "status" || *s == "statuses")?;
    let id = segs.get(pos + 1)?;
    id.bytes().all(|b| b.is_ascii_digit()).then(|| (*id).to_owned())
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TwitterExtractor;

#[async_trait]
impl PlatformExtractor for TwitterExtractor {
    fn platform(&self) -> Platform {
        Platform::Twitter
    }

    fn matches(&self, url: &Url) -> bool {
        ["x.com", "twitter.com", "mobile.twitter.com"].iter().any(|d| host_is(url, d))
    }

    async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
        let id = status_id(url).ok_or_else(|| CoreError::UnsupportedUrl(url.to_string()))?;
        let mut api = Url::parse("https://cdn.syndication.twimg.com/tweet-result")?;
        api.query_pairs_mut()
            .append_pair("id", &id)
            .append_pair("lang", "en")
            .append_pair("token", &syndication_token(&id));
        let tweet: Value = ctx.http.send_with_retry(|pool| pool.get(&api, DeviceClass::Desktop)).await?.json().await?;
        Ok(Extraction::Media(Box::new(parse_tweet(&tweet, url)?)))
    }
}

/// The syndication endpoint expects `((id / 1e15) * π)` rendered in base 36
/// without zeros or the decimal point.
fn syndication_token(id: &str) -> String {
    let n = id.parse::<f64>().unwrap_or(0.0) / 1e15 * std::f64::consts::PI;
    let (mut int, mut frac) = (n.trunc() as u64, n.fract());
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = Vec::new();
    loop {
        s.push(digits[(int % 36) as usize]);
        int /= 36;
        if int == 0 {
            break;
        }
    }
    s.reverse();
    for _ in 0..10 {
        frac *= 36.0;
        s.push(digits[frac.trunc() as usize % 36]);
        frac = frac.fract();
    }
    s.into_iter().filter(|c| *c != b'0').map(char::from).collect()
}

/// Accepts both the syndication shape (`mediaDetails`) and the GraphQL `legacy` shape.
pub fn parse_tweet(tweet: &Value, source: &Url) -> Result<MediaInfo> {
    let media = at(tweet, "mediaDetails")
        .or_else(|| at(tweet, "legacy/extended_entities/media"))
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::extraction(NAME, "post has no media"))?;
    let video = media
        .iter()
        .find(|m| matches!(at_str(m, "type"), Some("video" | "animated_gif")))
        .ok_or_else(|| CoreError::extraction(NAME, "post has no video"))?;

    let mut formats: Vec<StreamFormat> = at(video, "video_info/variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|v| at_str(v, "content_type") == Some("video/mp4"))
        .filter_map(|v| {
            let url = Url::parse(at_str(v, "url")?).ok()?;
            // Resolution is encoded in the path: /vid/avc1/1920x1080/xyz.mp4
            let (w, h) = url
                .path_segments()?
                .find_map(|seg| seg.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
                .map_or((None, None), |(a, b): (u32, u32)| (Some(a), Some(b)));
            Some(StreamFormat {
                id: at_u64(v, "bitrate").map_or_else(|| "mp4".into(), |b| format!("mp4-{b}")),
                url,
                kind: StreamKind::Muxed,
                transport: Transport::Http { content_length: None },
                container: "mp4".into(),
                width: w,
                height: h,
                fps: None,
                video_codec: Some(VideoCodec::H264),
                audio_codec: None,
                bitrate: at_u64(v, "bitrate"),
                hdr: false,
                headers: Vec::new(),
            })
        })
        .collect();
    formats.sort_by_key(|f| std::cmp::Reverse(f.bitrate.unwrap_or(0)));
    if formats.is_empty() {
        return Err(CoreError::extraction(NAME, "no MP4 variants available"));
    }

    let text = at_str(tweet, "text").or_else(|| at_str(tweet, "legacy/full_text")).unwrap_or("X video");
    Ok(MediaInfo {
        platform: Platform::Twitter,
        id: at_str(tweet, "id_str").or_else(|| at_str(tweet, "rest_id")).unwrap_or_default().to_owned(),
        source_url: source.clone(),
        title: text.to_owned(),
        author: at_str(tweet, "user/screen_name")
            .or_else(|| at_str(tweet, "core/user_results/result/legacy/screen_name"))
            .map(str::to_owned),
        thumbnail: at_str(video, "media_url_https").and_then(|u| Url::parse(u).ok()),
        duration_secs: at_u64(video, "video_info/duration_millis").map(|ms| ms as f64 / 1000.0),
        published_at: at_str(tweet, "created_at")
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.with_timezone(&chrono::Utc)),
        content_kind: ContentKind::Video,
        formats,
        subtitles: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_highest_bitrate_first() {
        let tweet = json!({
            "id_str": "1800000000000000000", "text": "Launch day 🚀", "user": { "screen_name": "acme" },
            "mediaDetails": [{ "type": "video", "media_url_https": "https://pbs.twimg.com/t.jpg",
                "video_info": { "duration_millis": 30000, "variants": [
                    { "content_type": "application/x-mpegURL", "url": "https://video.twimg.com/pl.m3u8" },
                    { "content_type": "video/mp4", "bitrate": 832000, "url": "https://video.twimg.com/vid/avc1/640x360/a.mp4" },
                    { "content_type": "video/mp4", "bitrate": 10368000, "url": "https://video.twimg.com/vid/avc1/1920x1080/b.mp4" }
            ]}}]
        });
        let src = Url::parse("https://x.com/acme/status/1800000000000000000").expect("url");
        let info = parse_tweet(&tweet, &src).expect("parsed");
        assert_eq!(info.formats.len(), 2);
        assert_eq!(info.formats[0].height, Some(1080));
        assert_eq!(info.duration_secs, Some(30.0));
    }

    #[test]
    fn extracts_status_id() {
        let u = Url::parse("https://x.com/acme/status/1800000000000000000/video/1").expect("url");
        assert_eq!(status_id(&u).as_deref(), Some("1800000000000000000"));
        assert!(!syndication_token("1800000000000000000").is_empty());
    }
}

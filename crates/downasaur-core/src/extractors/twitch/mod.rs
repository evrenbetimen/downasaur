//! Twitch live channels, VODs and clips via HLS.
//!
//! Flow: request a playback access token from the public GraphQL endpoint, build
//! the Usher master playlist URL, then map each `#EXT-X-STREAM-INF` variant to a
//! [`StreamFormat`] with [`Transport::Hls`]. The downloader's HLS module fetches
//! VOD segments concurrently or follows a live media playlist in real time.

use async_trait::async_trait;
use serde_json::{Value, json};
use url::Url;

use super::util::{at, at_str};
use super::{ExtractContext, PlatformExtractor, host_is};
use crate::drm;
use crate::error::{CoreError, Result};
use crate::model::{
    AudioCodec, ContentKind, Extraction, MediaInfo, Platform, StreamFormat, StreamKind, Transport, VideoCodec,
};
use crate::net::fingerprint::DeviceClass;

const NAME: &str = "Twitch";
const GQL: &str = "https://gql.twitch.tv/gql";
/// Public client ID used by the twitch.tv web player.
const WEB_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TwitchUrl {
    Live { login: String },
    Vod { id: String },
}

pub fn classify(url: &Url) -> Option<TwitchUrl> {
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        ["videos", id, ..] => Some(TwitchUrl::Vod { id: (*id).to_owned() }),
        [_, "video", id, ..] => Some(TwitchUrl::Vod { id: (*id).to_owned() }),
        [login] if !matches!(*login, "directory" | "settings" | "downloads") => {
            Some(TwitchUrl::Live { login: (*login).to_ascii_lowercase() })
        }
        _ => None,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TwitchExtractor;

#[async_trait]
impl PlatformExtractor for TwitchExtractor {
    fn platform(&self) -> Platform {
        Platform::Twitch
    }

    fn matches(&self, url: &Url) -> bool {
        host_is(url, "twitch.tv")
    }

    async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
        let target = classify(url).ok_or_else(|| CoreError::UnsupportedUrl(url.to_string()))?;
        let (is_live, login, vod_id) = match &target {
            TwitchUrl::Live { login } => (true, login.as_str(), ""),
            TwitchUrl::Vod { id } => (false, "", id.as_str()),
        };

        let gql = Url::parse(GQL)?;
        let body = json!({
            "operationName": "PlaybackAccessToken_Template",
            "query": "query PlaybackAccessToken_Template($login: String!, $isLive: Boolean!, $vodID: ID!, $isVod: Boolean!, $playerType: String!) { streamPlaybackAccessToken(channelName: $login, params: {platform: \"web\", playerBackend: \"mediaplayer\", playerType: $playerType}) @include(if: $isLive) { value signature } videoPlaybackAccessToken(id: $vodID, params: {platform: \"web\", playerBackend: \"mediaplayer\", playerType: $playerType}) @include(if: $isVod) { value signature } }",
            "variables": { "isLive": is_live, "login": login, "isVod": !is_live, "vodID": vod_id, "playerType": "site" }
        });
        let token: Value = ctx
            .http
            .send_with_retry(|pool| pool.client().post(gql.clone()).header("Client-ID", WEB_CLIENT_ID).json(&body))
            .await?
            .json()
            .await?;
        let tok_path = if is_live { "data/streamPlaybackAccessToken" } else { "data/videoPlaybackAccessToken" };
        let tok = at(&token, tok_path)
            .ok_or_else(|| CoreError::extraction(NAME, "channel is offline or VOD is unavailable"))?;

        let mut master = match &target {
            TwitchUrl::Live { login } => Url::parse(&format!("https://usher.ttvnw.net/api/channel/hls/{login}.m3u8"))?,
            TwitchUrl::Vod { id } => Url::parse(&format!("https://usher.ttvnw.net/vod/{id}.m3u8"))?,
        };
        master
            .query_pairs_mut()
            .append_pair("sig", at_str(tok, "signature").unwrap_or_default())
            .append_pair("token", at_str(tok, "value").unwrap_or_default())
            .append_pair("allow_source", "true")
            .append_pair("allow_audio_only", "true")
            .append_pair("p", &fastrand::u32(100_000..999_999).to_string());

        let text = ctx.http.get_text(&master, DeviceClass::Desktop).await?;
        drm::ensure_clear(drm::detect_in_hls(&text))?;
        let formats = parse_master_playlist(&text, &master, is_live)?;

        let author = if login.is_empty() { "twitch".to_owned() } else { login.to_owned() };
        let (id, title, kind) = match target {
            TwitchUrl::Live { login } => (login.clone(), format!("{login} live"), ContentKind::LiveStream),
            TwitchUrl::Vod { id } => (id.clone(), format!("Twitch VOD {id}"), ContentKind::Vod),
        };
        Ok(Extraction::Media(Box::new(MediaInfo {
            platform: Platform::Twitch,
            author: Some(author),
            id,
            source_url: url.clone(),
            title,
            thumbnail: None,
            duration_secs: None,
            published_at: None,
            content_kind: kind,
            formats,
            subtitles: Vec::new(),
        })))
    }
}

/// Map an HLS master playlist's variants to formats. Reusable for any HLS source.
pub fn parse_master_playlist(text: &str, base: &Url, live: bool) -> Result<Vec<StreamFormat>> {
    let master = m3u8_rs::parse_master_playlist_res(text.as_bytes())
        .map_err(|e| CoreError::Parse(format!("invalid HLS master playlist: {e}")))?;
    Ok(master
        .variants
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.is_i_frame)
        .filter_map(|(i, v)| {
            let url = base.join(&v.uri).ok()?;
            let codecs = v.codecs.as_deref().unwrap_or("");
            let mut parts = codecs.split(',').map(str::trim);
            let first = parts.next().unwrap_or("");
            let audio_only = v.resolution.is_none() && (first.starts_with("mp4a") || first.starts_with("opus"));
            let (width, height) = v
                .resolution
                .map(|r| (u32::try_from(r.width).ok(), u32::try_from(r.height).ok()))
                .unwrap_or((None, None));
            Some(StreamFormat {
                id: v.video.clone().unwrap_or_else(|| format!("hls-{i}")),
                url,
                kind: if audio_only { StreamKind::AudioOnly } else { StreamKind::Muxed },
                transport: Transport::Hls { live },
                container: "ts".into(),
                width,
                height,
                fps: v.frame_rate.map(|f| f as f32),
                video_codec: (!audio_only).then(|| VideoCodec::from_codecs_attr(first)),
                audio_codec: if audio_only {
                    Some(AudioCodec::from_codecs_attr(first))
                } else {
                    parts.next().map(AudioCodec::from_codecs_attr)
                },
                bitrate: v.average_bandwidth.or(Some(v.bandwidth)),
                hdr: false,
                headers: Vec::new(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=8500000,RESOLUTION=1920x1080,CODECS=\"avc1.64002A,mp4a.40.2\",VIDEO=\"chunked\",FRAME-RATE=60.000
chunked/index-dvr.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1280x720,CODECS=\"avc1.4D401F,mp4a.40.2\",VIDEO=\"720p60\",FRAME-RATE=60.000
720p60/index-dvr.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=160000,CODECS=\"mp4a.40.2\",VIDEO=\"audio_only\"
audio_only/index-dvr.m3u8
";

    #[test]
    fn parses_variants() {
        let base = Url::parse("https://d1m7jfoe9zdc1j.cloudfront.net/abc/").expect("url");
        let f = parse_master_playlist(MASTER, &base, false).expect("parsed");
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].id, "chunked");
        assert_eq!(f[0].height, Some(1080));
        assert_eq!(f[0].url.as_str(), "https://d1m7jfoe9zdc1j.cloudfront.net/abc/chunked/index-dvr.m3u8");
        assert_eq!(f[2].kind, StreamKind::AudioOnly);
    }

    #[test]
    fn classifies() {
        let u = |s: &str| Url::parse(s).expect("url");
        assert_eq!(classify(&u("https://www.twitch.tv/videos/123")), Some(TwitchUrl::Vod { id: "123".into() }));
        assert_eq!(classify(&u("https://www.twitch.tv/Shroud")), Some(TwitchUrl::Live { login: "shroud".into() }));
    }
}

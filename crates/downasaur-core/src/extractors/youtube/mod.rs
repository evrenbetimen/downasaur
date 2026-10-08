//! YouTube: videos, Shorts, live streams, playlists and channels.
//!
//! Adaptive formats go up to 4320p60 (AV1 itag 571/402, VP9 itag 272/337). Video and
//! audio arrive as separate DASH-style streams and are joined by the remux stage.
//!
//! Sub-modules:
//! - [`urls`]: URL classification (watch / shorts / live / playlist / channel).
//! - [`formats`]: `streamingData` → [`StreamFormat`] mapping and best-format selection.
//! - [`cipher`]: `signatureCipher` unpacking.
//! - [`jsc`]: player-script challenge solver (`n` throttling + signatures).
//! - [`playlist`]: playlist/channel crawling with continuation tokens.

pub mod cipher;
pub mod formats;
pub mod jsc;
pub mod playlist;
pub mod urls;

use std::sync::LazyLock;

use async_trait::async_trait;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Value, json};
use url::Url;

use super::util::{at, at_str, at_u64};
use super::{ExtractContext, PlatformExtractor, host_is};
use crate::cookies::header_value;
use crate::error::{CoreError, DrmScheme, Result};
use crate::model::{ContentKind, Extraction, MediaInfo, Platform, SubtitleTrack};
use crate::net::fingerprint::DeviceClass;
use urls::YouTubeUrl;

const NAME: &str = "YouTube";
const PLAYER_ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";

const ORIGIN: &str = "https://www.youtube.com";

/// Visitor ids shared by the player requests of this process, anonymous and
/// signed-in (`[anon, authed]`); see [`visitor_data`].
static VISITOR_DATA: LazyLock<Mutex<[Option<String>; 2]>> = LazyLock::new(Default::default);

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
                extract_video(ctx, url, &id, kind).await.map(|m| Extraction::Media(Box::new(m)))
            }
            Some(YouTubeUrl::Playlist { list }) => playlist::crawl_playlist(ctx, &list).await,
            Some(YouTubeUrl::Channel { path }) => playlist::crawl_channel(ctx, &path).await,
            None => Err(CoreError::UnsupportedUrl(url.to_string())),
        }
    }
}

/// An InnerTube client identity. Each client gets a different set of formats and
/// different protection: some need the web player's JS transforms, some don't.
#[derive(Debug, Clone, Copy)]
pub struct InnerTubeClient {
    pub name: &'static str,
    /// Numeric id sent as `X-YouTube-Client-Name`.
    pub id: u32,
    pub version: &'static str,
    pub user_agent: &'static str,
    pub extra: &'static [(&'static str, &'static str)],
    /// Stream URLs carry `n` (and possibly `signatureCipher`) challenges that need
    /// the player script.
    pub needs_player_js: bool,
    pub cookies: CookieUse,
    /// Sends `context.thirdParty.embedUrl`, as an embedded player does.
    pub embedded: bool,
}

/// How a client treats imported YouTube cookies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieUse {
    /// Never sent (the client is anonymous-only).
    Never,
    /// Sent when imported; the client also works without.
    Optional,
    /// The client is only tried when cookies were imported.
    Required,
}

/// visionOS returns adaptive formats with plain URLs and no proof-of-origin token
/// requirement. TV is the fallback (e.g. for videos visionOS refuses); its URLs
/// need the `n`/signature transforms from the player script.
pub const CLIENTS: &[InnerTubeClient] = &[
    InnerTubeClient {
        name: "VISIONOS",
        id: 101,
        version: "1.02",
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
        extra: &[
            ("deviceMake", "Apple"),
            ("deviceModel", "RealityDevice17,1"),
            ("osName", "visionOS"),
            ("osVersion", "26.5.23O471"),
        ],
        needs_player_js: false,
        cookies: CookieUse::Never,
        embedded: false,
    },
    InnerTubeClient {
        name: "WEB_EMBEDDED_PLAYER",
        id: 56,
        version: "1.20260708.00.00",
        user_agent: WEB_USER_AGENT,
        extra: &[],
        needs_player_js: true,
        cookies: CookieUse::Required,
        embedded: true,
    },
    InnerTubeClient {
        name: "TVHTML5",
        id: 7,
        version: "5.20260707",
        user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
        extra: &[],
        needs_player_js: true,
        cookies: CookieUse::Optional,
        embedded: false,
    },
    InnerTubeClient {
        name: "WEB",
        id: 1,
        version: "2.20261007.01.00",
        user_agent: WEB_USER_AGENT,
        extra: &[],
        needs_player_js: true,
        cookies: CookieUse::Required,
        embedded: false,
    },
];

const WEB_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// Clients to try, in order; [`CookieUse::Required`] clients only when signed in.
/// `DOWNASAUR_YT_CLIENTS=TVHTML5` narrows and reorders the chain (for debugging
/// a single client).
fn client_chain(signed_in: bool) -> Vec<&'static InnerTubeClient> {
    let usable = |c: &&InnerTubeClient| signed_in || c.cookies != CookieUse::Required;
    let only = std::env::var("DOWNASAUR_YT_CLIENTS").unwrap_or_default();
    let chain: Vec<_> = only
        .split(',')
        .filter_map(|n| CLIENTS.iter().find(|c| c.name.eq_ignore_ascii_case(n.trim())))
        .filter(usable)
        .collect();
    if chain.is_empty() { CLIENTS.iter().filter(usable).collect() } else { chain }
}

async fn extract_video(ctx: &ExtractContext, source: &Url, id: &str, kind: ContentKind) -> Result<MediaInfo> {
    let mut info = extract_with_clients(ctx, source, id, kind).await?;
    // visionOS answers without `microformat`; the organizer files by publish date.
    if info.published_at.is_none() && info.content_kind != ContentKind::LiveStream {
        info.published_at = fetch_publish_date(ctx, id).await;
    }
    Ok(info)
}

/// Publish date from the watch page (`<meta itemprop="datePublished">` or the
/// inline `publishDate`). Best effort: `None` on any failure.
async fn fetch_publish_date(ctx: &ExtractContext, id: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let url = Url::parse_with_params("https://www.youtube.com/watch", [("v", id), ("hl", "en")]).ok()?;
    match ctx.http.get_text(&url, DeviceClass::Desktop).await {
        Ok(html) => parse_publish_date(&html),
        Err(e) => {
            tracing::debug!(error = %e, "youtube publish date fetch failed");
            None
        }
    }
}

fn parse_publish_date(html: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"itemprop="datePublished" content="([^"]+)"|"publishDate":"([^"]+)""#).expect("valid regex")
    });
    RE.captures_iter(html)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .find_map(|m| chrono::DateTime::parse_from_rfc3339(m.as_str()).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Try each client in the chain and return the first full-tier result, else the
/// best degraded one (with a warning).
async fn extract_with_clients(ctx: &ExtractContext, source: &Url, id: &str, kind: ContentKind) -> Result<MediaInfo> {
    let cookie = ctx.cookie_for(Platform::YouTube).filter(|c| sapisid(c).is_some());
    let mut first_err = None;
    // Best result so far from a client that could not reach the listed top tier.
    let mut degraded: Option<MediaInfo> = None;
    for client in client_chain(cookie.is_some()) {
        let cookie = cookie.filter(|_| client.cookies != CookieUse::Never);
        let visitor = visitor_data(ctx, cookie).await;
        match extract_with_client(ctx, source, id, kind, client, visitor.as_deref(), cookie).await {
            Ok(info) if !info.formats.is_empty() && info.warnings.is_empty() => {
                tracing::debug!(client = client.name, formats = info.formats.len(), "youtube client succeeded");
                return Ok(info);
            }
            Ok(info) if !info.formats.is_empty() => {
                tracing::debug!(client = client.name, warning = ?info.warnings, "youtube client degraded; trying next");
                if degraded.as_ref().is_none_or(|d| info.max_height() > d.max_height()) {
                    degraded = Some(info);
                }
            }
            Ok(_) => tracing::debug!(client = client.name, "youtube client returned no formats"),
            // DRM is a property of the title, not of the client: report it right away.
            Err(e @ CoreError::DrmProtected(_)) => return Err(e),
            Err(e) => {
                tracing::debug!(client = client.name, error = %e, "youtube client failed");
                first_err.get_or_insert(e);
            }
        }
    }
    if let Some(info) = degraded {
        tracing::warn!(warnings = ?info.warnings, "youtube: only a lower tier is downloadable");
        return Ok(info);
    }
    Err(first_err.unwrap_or_else(|| CoreError::FormatUnavailable("no downloadable YouTube formats".into())))
}

/// Tallest video the player response lists, downloadable or not (formats without
/// a URL are SABR-only or ciphered).
fn listed_max_height(player: &Value) -> Option<u32> {
    at(player, "streamingData/adaptiveFormats")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|f| at_u64(f, "height").and_then(|h| u32::try_from(h).ok()))
        .max()
}

/// A warning when the downloadable formats stop well below what the video offers.
fn tier_warning(client: &str, listed: Option<u32>, usable: Option<u32>) -> Option<String> {
    let listed = listed?;
    let usable = usable.unwrap_or(0);
    (usable < listed).then(|| {
        format!("YouTube lists up to {listed}p for this video, but the {client} client could only get {usable}p")
    })
}

async fn extract_with_client(
    ctx: &ExtractContext,
    source: &Url,
    id: &str,
    kind: ContentKind,
    client: &InnerTubeClient,
    visitor: Option<&str>,
    cookie: Option<&str>,
) -> Result<MediaInfo> {
    let script = if client.needs_player_js { Some(jsc::PlayerScript::load(&ctx.http).await?) } else { None };
    let sts = script.as_ref().and_then(|s| s.signature_timestamp);
    let player = fetch_player_response(ctx, id, client, sts, visitor, cookie).await?;
    let mut info = parse_player_response(&player, source, kind)?;
    let hls_manifest = at_str(&player, "streamingData/hlsManifestUrl").and_then(|u| Url::parse(u).ok());
    match hls_manifest {
        // Live adaptive URLs are per-segment (`sq=`) endpoints, not files: record
        // the HLS playlists instead.
        Some(manifest) if info.content_kind == ContentKind::LiveStream => {
            let master = ctx.http.get_text(&manifest, DeviceClass::Desktop).await?;
            info.formats = formats::parse_hls_master(&master, &manifest)?;
        }
        _ => {
            if let Some(script) = &script {
                formats::solve_challenges(&mut info, &player, script).await?;
            }
            info.warnings.extend(tier_warning(client.name, listed_max_height(&player), info.max_height()));
        }
    }
    // googlevideo checks that the downloader looks like the client that asked.
    for f in &mut info.formats {
        f.headers.push(("User-Agent".into(), client.user_agent.into()));
    }
    Ok(info)
}

/// A visitor id (`VISITOR_DATA` from the homepage's `ytcfg`). Anonymous player
/// requests without one are answered with "Sign in to confirm you're not a bot",
/// even from residential IPs. Signed-in requests use the id of their own session.
/// Fetched once per process and kind; `None` if the page changed.
async fn visitor_data(ctx: &ExtractContext, cookie: Option<&str>) -> Option<String> {
    let slot = usize::from(cookie.is_some());
    if let Some(v) = VISITOR_DATA.lock()[slot].clone() {
        return Some(v);
    }
    let home = Url::parse("https://www.youtube.com/").ok()?;
    let fetched = ctx
        .http
        .send_with_retry(|pool| {
            let req = pool.get(&home, DeviceClass::Desktop);
            match cookie {
                Some(c) => req.header("Cookie", c),
                None => req,
            }
        })
        .await;
    let html = match fetched {
        Ok(resp) => resp.text().await.map_err(CoreError::from),
        Err(e) => Err(e),
    };
    let html = match html {
        Ok(html) => html,
        Err(e) => {
            tracing::debug!(error = %e, "youtube visitor data fetch failed");
            return None;
        }
    };
    let visitor = parse_visitor_data(&html);
    if visitor.is_none() {
        tracing::debug!("youtube homepage carried no VISITOR_DATA");
    }
    VISITOR_DATA.lock()[slot].clone_from(&visitor);
    visitor
}

/// The `SAPISID` session cookie (or its `__Secure-3PAPISID` twin) that signs
/// authenticated InnerTube requests.
fn sapisid(cookie: &str) -> Option<&str> {
    header_value(cookie, "SAPISID").or_else(|| header_value(cookie, "__Secure-3PAPISID"))
}

/// `Authorization` value for a signed-in request, as the web client computes it:
/// `<scheme> <ts>_<sha1("<ts> <sid> <origin>")>` for each session id present.
fn sapisid_authorization(cookie: &str, origin: &str, now: i64) -> Option<String> {
    let schemes = [
        ("SAPISIDHASH", sapisid(cookie)),
        ("SAPISID1PHASH", header_value(cookie, "__Secure-1PAPISID")),
        ("SAPISID3PHASH", header_value(cookie, "__Secure-3PAPISID")),
    ];
    let parts: Vec<String> = schemes
        .iter()
        .filter_map(|(scheme, sid)| {
            let digest = sha1_smol::Sha1::from(format!("{now} {} {origin}", (*sid)?)).digest().to_string();
            Some(format!("{scheme} {now}_{digest}"))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn parse_visitor_data(html: &str) -> Option<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""VISITOR_DATA":"([^"]+)""#).expect("valid regex"));
    RE.captures(html).map(|c| c[1].to_owned())
}

/// Query the InnerTube player endpoint for a video's `playerResponse`.
async fn fetch_player_response(
    ctx: &ExtractContext,
    video_id: &str,
    client: &InnerTubeClient,
    signature_timestamp: Option<u64>,
    visitor: Option<&str>,
    cookie: Option<&str>,
) -> Result<Value> {
    let endpoint = Url::parse(PLAYER_ENDPOINT)?;
    let body = player_request_body(video_id, client, signature_timestamp, visitor);
    let resp = ctx
        .http
        .send_with_retry(|pool| {
            let req = pool
                .client()
                .post(endpoint.clone())
                .json(&body)
                .header("User-Agent", client.user_agent)
                .header("X-YouTube-Client-Name", client.id.to_string())
                .header("X-YouTube-Client-Version", client.version)
                .header("Origin", ORIGIN);
            let req = match visitor {
                Some(v) => req.header("X-Goog-Visitor-Id", v),
                None => req,
            };
            // Signed again on every attempt: the hash embeds the current time.
            match cookie.and_then(|c| Some((c, sapisid_authorization(c, ORIGIN, chrono::Utc::now().timestamp())?))) {
                Some((c, auth)) => req
                    .header("Cookie", c)
                    .header("Authorization", auth)
                    .header("X-Origin", ORIGIN)
                    .header("X-Goog-AuthUser", "0")
                    .header("X-Youtube-Bootstrap-Logged-In", "true"),
                None => req,
            }
        })
        .await?;
    Ok(resp.json().await?)
}

fn player_request_body(
    video_id: &str,
    client: &InnerTubeClient,
    signature_timestamp: Option<u64>,
    visitor: Option<&str>,
) -> Value {
    let mut client_ctx = json!({ "clientName": client.name, "clientVersion": client.version, "hl": "en", "userAgent": client.user_agent });
    for (k, v) in client.extra {
        client_ctx[*k] = json!(v);
    }
    if let Some(v) = visitor {
        client_ctx["visitorData"] = json!(v);
    }
    let mut context = json!({ "client": client_ctx });
    if client.embedded {
        context["thirdParty"] = json!({ "embedUrl": format!("{ORIGIN}/") });
    }
    let mut playback = json!({ "html5Preference": "HTML5_PREF_WANTS" });
    if let Some(sts) = signature_timestamp {
        playback["signatureTimestamp"] = json!(sts);
    }
    json!({
        "videoId": video_id,
        "context": context,
        "playbackContext": { "contentPlaybackContext": playback },
        "contentCheckOk": true,
        "racyCheckOk": true
    })
}

/// Turn an InnerTube `playerResponse` into [`MediaInfo`].
///
/// Formats that still carry a `signatureCipher` keep their raw base URL here;
/// [`formats::solve_challenges`] adds them once the player transforms are solved.
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
        warnings: Vec::new(),
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
    fn request_body_carries_client_identity() {
        let body = player_request_body("abcdefghijk", &CLIENTS[0], None, Some("CgtWaXNpdG9y"));
        assert_eq!(body["context"]["client"]["clientName"], "VISIONOS");
        assert_eq!(body["context"]["client"]["deviceModel"], "RealityDevice17,1");
        assert_eq!(body["context"]["client"]["visitorData"], "CgtWaXNpdG9y");
        assert!(body["playbackContext"]["contentPlaybackContext"].get("signatureTimestamp").is_none());
        let body = player_request_body("abcdefghijk", &CLIENTS[2], Some(20367), None);
        assert_eq!(body["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"], 20367);
        assert!(body["context"]["client"].get("visitorData").is_none());
    }

    #[test]
    fn sapisid_hash_matches_web_client() {
        let cookie = "PREF=x; SAPISID=abc/def; __Secure-1PAPISID=one; __Secure-3PAPISID=three";
        assert_eq!(
            sapisid_authorization(cookie, ORIGIN, 1_700_000_000).as_deref(),
            Some(
                "SAPISIDHASH 1700000000_747622f274182ecf105054645d6a0093199ab03d \
                 SAPISID1PHASH 1700000000_01ca8712a198105a3f0999f890c24137388b7fff \
                 SAPISID3PHASH 1700000000_7f5cf0ef7325a890a89cc0dc432ce24c7bae42dc"
            )
        );
        // __Secure-3PAPISID stands in for a missing SAPISID.
        let auth = sapisid_authorization("__Secure-3PAPISID=three", ORIGIN, 1_700_000_000).expect("auth");
        assert!(auth.starts_with("SAPISIDHASH 1700000000_7f5cf0ef"), "{auth}");
        assert_eq!(sapisid_authorization("PREF=x", ORIGIN, 1_700_000_000), None);
    }

    #[test]
    fn signed_out_chain_skips_cookie_only_clients() {
        let names = |signed_in| client_chain(signed_in).iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names(false), ["VISIONOS", "TVHTML5"]);
        assert_eq!(names(true), ["VISIONOS", "WEB_EMBEDDED_PLAYER", "TVHTML5", "WEB"]);
        let body = player_request_body("abcdefghijk", &CLIENTS[1], None, None);
        assert_eq!(body["context"]["thirdParty"]["embedUrl"], "https://www.youtube.com/");
    }

    #[test]
    fn warns_when_only_a_lower_tier_is_downloadable() {
        let mut p = sample_player();
        assert_eq!(listed_max_height(&p), Some(4320));
        // The 8K formats lose their URLs (SABR-only), 360p stays.
        for f in p["streamingData"]["adaptiveFormats"].as_array_mut().expect("formats") {
            if f["itag"] != 251 {
                f.as_object_mut().expect("obj").remove("url");
            }
        }
        p["streamingData"]["formats"] = json!([{ "itag": 18, "url": "https://rr.googlevideo.com/a?itag=18",
            "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"", "width": 640, "height": 360 }]);
        let src = Url::parse("https://youtu.be/abcdefghijk").expect("url");
        let info = parse_player_response(&p, &src, ContentKind::Video).expect("parsed");
        let warning = tier_warning("WEB", listed_max_height(&p), info.max_height()).expect("warning");
        assert_eq!(warning, "YouTube lists up to 4320p for this video, but the WEB client could only get 360p");
        assert_eq!(tier_warning("VISIONOS", Some(4320), Some(4320)), None);
        assert_eq!(tier_warning("VISIONOS", None, Some(720)), None);
    }

    #[test]
    fn publish_date_from_watch_page() {
        let html =
            r#"<meta itemprop="name" content="x"><meta itemprop="datePublished" content="2019-09-01T08:04:14-07:00">"#;
        assert_eq!(parse_publish_date(html).map(|d| d.to_rfc3339()).as_deref(), Some("2019-09-01T15:04:14+00:00"));
        let html = r#"{"microformat":{"publishDate":"2024-02-29T00:00:00+00:00"}}"#;
        assert_eq!(parse_publish_date(html).map(|d| d.date_naive().to_string()).as_deref(), Some("2024-02-29"));
        assert_eq!(parse_publish_date(r#"itemprop="datePublished" content="2019-09-01""#), None);
        assert_eq!(parse_publish_date("<html></html>"), None);
    }

    #[test]
    fn visitor_data_is_read_from_ytcfg() {
        let html = r#"<script>ytcfg.set({"INNERTUBE_API_KEY":"x","VISITOR_DATA":"CgtLeDdPd2Z0TzEtVSjX75vWBg%3D%3D","HL":"en"});</script>"#;
        assert_eq!(parse_visitor_data(html).as_deref(), Some("CgtLeDdPd2Z0TzEtVSjX75vWBg%3D%3D"));
        assert_eq!(parse_visitor_data("<html></html>"), None);
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

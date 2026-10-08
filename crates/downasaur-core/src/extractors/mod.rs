//! Platform extractors.
//!
//! Each platform implements [`PlatformExtractor`]. The [`ExtractorRegistry`]
//! routes an input URL to the first extractor whose [`PlatformExtractor::matches`]
//! returns `true`; the universal yt-dlp bridge is always registered last.

pub mod facebook;
pub mod generic;
pub mod instagram;
pub mod tiktok;
pub mod twitch;
pub mod twitter;
pub mod util;
pub mod youtube;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use url::Url;

use crate::error::{CoreError, Result};
use crate::model::{Extraction, Platform};
use crate::net::HttpPool;

/// Shared state handed to every extractor call.
#[derive(Debug, Clone)]
pub struct ExtractContext {
    pub http: HttpPool,
    /// Per-platform `Cookie` header values imported by the user (e.g. to access
    /// their own Instagram stories). Never required for public content.
    pub cookies: HashMap<Platform, String>,
    /// Path to the `yt-dlp` executable used by the universal fallback.
    pub yt_dlp: PathBuf,
}

impl ExtractContext {
    pub fn new(http: HttpPool) -> Self {
        Self { http, cookies: HashMap::new(), yt_dlp: PathBuf::from("yt-dlp") }
    }

    pub fn cookie_for(&self, platform: Platform) -> Option<&str> {
        self.cookies.get(&platform).map(String::as_str)
    }
}

#[async_trait]
pub trait PlatformExtractor: Send + Sync + std::fmt::Debug {
    fn platform(&self) -> Platform;

    /// Cheap, offline check used for routing and for the UI's link sniffer.
    fn matches(&self, url: &Url) -> bool;

    /// Resolve a URL into media metadata and stream formats, or a collection of
    /// entry URLs (playlists, channels) to be expanded by the caller.
    async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction>;
}

#[derive(Debug, Clone)]
pub struct ExtractorRegistry {
    extractors: Vec<Arc<dyn PlatformExtractor>>,
    fallback: Arc<dyn PlatformExtractor>,
}

impl Default for ExtractorRegistry {
    fn default() -> Self {
        Self {
            extractors: vec![
                Arc::new(youtube::YouTubeExtractor),
                Arc::new(tiktok::TikTokExtractor),
                Arc::new(instagram::InstagramExtractor),
                Arc::new(twitch::TwitchExtractor),
                Arc::new(twitter::TwitterExtractor),
                Arc::new(facebook::FacebookExtractor),
            ],
            fallback: Arc::new(generic::YtDlpExtractor),
        }
    }
}

impl ExtractorRegistry {
    /// Parse and normalize user input (trims whitespace, adds a missing scheme).
    pub fn parse_input(input: &str) -> Result<Url> {
        let trimmed = input.trim();
        let candidate = if trimmed.contains("://") { trimmed.to_owned() } else { format!("https://{trimmed}") };
        let url = Url::parse(&candidate)?;
        match url.scheme() {
            "http" | "https" => Ok(url),
            other => Err(CoreError::UnsupportedUrl(format!("unsupported scheme `{other}`"))),
        }
    }

    /// The extractor that will handle `url` (dedicated module or the fallback).
    pub fn route(&self, url: &Url) -> &Arc<dyn PlatformExtractor> {
        self.extractors.iter().find(|e| e.matches(url)).unwrap_or(&self.fallback)
    }

    /// Offline platform detection for the link sniffer.
    pub fn detect_platform(&self, url: &Url) -> Platform {
        self.route(url).platform()
    }

    pub async fn extract(&self, ctx: &ExtractContext, url: &Url) -> Result<Extraction> {
        let extractor = self.route(url);
        tracing::info!(platform = ?extractor.platform(), %url, "extracting");
        extractor.extract(ctx, url).await
    }
}

/// Host matching helper: `host` equals `domain` or is a subdomain of it.
pub(crate) fn host_is(url: &Url, domain: &str) -> bool {
    url.host_str().is_some_and(|h| {
        let h = h.trim_end_matches('.');
        h.eq_ignore_ascii_case(domain)
            || (h.len() > domain.len()
                && h[h.len() - domain.len()..].eq_ignore_ascii_case(domain)
                && h.as_bytes()[h.len() - domain.len() - 1] == b'.')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform_of(s: &str) -> Platform {
        let reg = ExtractorRegistry::default();
        reg.detect_platform(&ExtractorRegistry::parse_input(s).expect("valid url"))
    }

    #[test]
    fn routes_each_platform() {
        assert_eq!(platform_of("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), Platform::YouTube);
        assert_eq!(platform_of("youtu.be/dQw4w9WgXcQ"), Platform::YouTube);
        assert_eq!(platform_of("https://www.youtube.com/shorts/abcdefghijk"), Platform::YouTube);
        assert_eq!(platform_of("https://www.tiktok.com/@user/video/7300000000000000000"), Platform::TikTok);
        assert_eq!(platform_of("https://vm.tiktok.com/ZMabc123/"), Platform::TikTok);
        assert_eq!(platform_of("https://www.instagram.com/reel/Cxyz123/"), Platform::Instagram);
        assert_eq!(platform_of("https://www.twitch.tv/videos/123456789"), Platform::Twitch);
        assert_eq!(platform_of("https://x.com/user/status/1800000000000000000"), Platform::Twitter);
        assert_eq!(platform_of("https://twitter.com/user/status/1800000000000000000"), Platform::Twitter);
        assert_eq!(platform_of("https://www.facebook.com/watch/?v=1234567890"), Platform::Facebook);
        assert_eq!(platform_of("https://vimeo.com/76979871"), Platform::Generic);
    }

    #[test]
    fn host_matching_rejects_lookalikes() {
        let u = Url::parse("https://notyoutube.com/watch?v=x").expect("valid");
        assert!(!host_is(&u, "youtube.com"));
        let u = Url::parse("https://m.youtube.com/watch?v=x").expect("valid");
        assert!(host_is(&u, "youtube.com"));
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(ExtractorRegistry::parse_input("file:///etc/passwd").is_err());
    }
}

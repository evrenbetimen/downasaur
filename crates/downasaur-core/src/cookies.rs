//! User-supplied cookies, imported from a Netscape `cookies.txt` file (the format
//! written by browser "Get cookies.txt" extensions and by `yt-dlp --cookies`).
//!
//! Cookies are only ever sent to the platform they belong to and never logged:
//! [`PlatformCookies`] redacts its values in `Debug` output.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{CoreError, Result};
use crate::model::Platform;

/// One cookie line from a `cookies.txt` file.
#[derive(Clone, PartialEq, Eq)]
pub struct Cookie {
    /// Domain without the leading dot, lowercased.
    pub domain: String,
    pub include_subdomains: bool,
    pub path: String,
    pub secure: bool,
    /// Unix seconds; `0` for session cookies.
    pub expires: i64,
    pub name: String,
    pub value: String,
}

impl std::fmt::Debug for Cookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cookie").field("domain", &self.domain).field("name", &self.name).finish_non_exhaustive()
    }
}

impl Cookie {
    /// Whether this cookie is sent to `host` (a bare host name).
    pub fn matches_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        host == self.domain || (self.include_subdomains && host.ends_with(&format!(".{}", self.domain)))
    }
}

/// Parse a Netscape `cookies.txt` document. Comments, blank lines and cookies that
/// expired before `now` (Unix seconds) are skipped; `#HttpOnly_` lines are kept.
pub fn parse_netscape(text: &str, now: i64) -> Result<Vec<Cookie>> {
    let mut cookies = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let [domain, include_subdomains, path, secure, expires, name, value] = fields[..] else {
            return Err(CoreError::Parse(format!("cookies.txt line {}: expected 7 tab-separated fields", i + 1)));
        };
        let expires: i64 = expires
            .trim()
            .parse::<f64>()
            .map(|e| e as i64)
            .map_err(|_| CoreError::Parse(format!("cookies.txt line {}: bad expiry", i + 1)))?;
        if expires != 0 && expires < now {
            continue;
        }
        cookies.push(Cookie {
            domain: domain.trim_start_matches('.').to_ascii_lowercase(),
            include_subdomains: include_subdomains.eq_ignore_ascii_case("TRUE") || domain.starts_with('.'),
            path: path.to_owned(),
            secure: secure.eq_ignore_ascii_case("TRUE"),
            expires,
            name: name.to_owned(),
            value: value.to_owned(),
        });
    }
    Ok(cookies)
}

/// The host whose cookies a platform's requests carry.
const fn platform_host(platform: Platform) -> Option<&'static str> {
    match platform {
        Platform::YouTube => Some("www.youtube.com"),
        Platform::TikTok => Some("www.tiktok.com"),
        Platform::Instagram => Some("www.instagram.com"),
        Platform::Twitch => Some("www.twitch.tv"),
        Platform::Twitter => Some("x.com"),
        Platform::Facebook => Some("www.facebook.com"),
        Platform::Generic => None,
    }
}

/// `Cookie` header values per platform. `Debug` never prints the values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PlatformCookies(HashMap<Platform, String>);

impl std::fmt::Debug for PlatformCookies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut platforms: Vec<_> = self.0.keys().map(|p| p.display_name()).collect();
        platforms.sort_unstable();
        f.debug_tuple("PlatformCookies").field(&platforms).finish()
    }
}

impl PlatformCookies {
    /// Group parsed cookies into one `Cookie` header per platform.
    pub fn from_cookies(cookies: &[Cookie]) -> Self {
        let platforms = [
            Platform::YouTube,
            Platform::TikTok,
            Platform::Instagram,
            Platform::Twitch,
            Platform::Twitter,
            Platform::Facebook,
        ];
        let mut map = HashMap::new();
        for platform in platforms {
            let Some(host) = platform_host(platform) else { continue };
            let header = cookies
                .iter()
                .filter(|c| c.matches_host(host))
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; ");
            if !header.is_empty() {
                map.insert(platform, header);
            }
        }
        Self(map)
    }

    /// Read and parse a `cookies.txt` file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_cookies(&parse_netscape(&text, chrono::Utc::now().timestamp())?))
    }

    pub fn insert(&mut self, platform: Platform, header: String) {
        self.0.insert(platform, header);
    }

    pub fn get(&self, platform: Platform) -> Option<&str> {
        self.0.get(&platform).map(String::as_str)
    }

    pub fn platforms(&self) -> impl Iterator<Item = Platform> + '_ {
        self.0.keys().copied()
    }
}

/// Value of cookie `name` inside a `Cookie` header string.
pub fn header_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').filter_map(|kv| kv.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Netscape HTTP Cookie File\n\
        # https://curl.se/docs/http-cookies.html\n\
        \n\
        .youtube.com\tTRUE\t/\tTRUE\t1893456000\tSAPISID\tabc/def\n\
        #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1893456000\tLOGIN_INFO\tlogin\n\
        .youtube.com\tTRUE\t/\tFALSE\t1000\tOLD\texpired\n\
        www.youtube.com\tFALSE\t/\tFALSE\t0\tPREF\tf6=40000000\n\
        .google.com\tTRUE\t/\tTRUE\t1893456000\tSID\tgoogle-only\n\
        .instagram.com\tTRUE\t/\tTRUE\t1893456000.5\tsessionid\tig\r\n";

    #[test]
    fn parses_netscape_file() {
        let cookies = parse_netscape(SAMPLE, 1_800_000_000).expect("parsed");
        let names: Vec<_> = cookies.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["SAPISID", "LOGIN_INFO", "PREF", "SID", "sessionid"]);
        assert_eq!(cookies[0].domain, "youtube.com");
        assert!(cookies[0].include_subdomains && cookies[0].secure);
        assert_eq!(cookies[2].expires, 0);
        assert_eq!(cookies[4].value, "ig");
    }

    #[test]
    fn rejects_malformed_lines() {
        let err = parse_netscape("youtube.com\tTRUE\t/\n", 0).expect_err("short line");
        assert!(err.to_string().contains("line 1"), "{err}");
    }

    #[test]
    fn groups_by_platform() {
        let jar = PlatformCookies::from_cookies(&parse_netscape(SAMPLE, 1_800_000_000).expect("parsed"));
        assert_eq!(jar.get(Platform::YouTube), Some("SAPISID=abc/def; LOGIN_INFO=login; PREF=f6=40000000"));
        assert_eq!(jar.get(Platform::Instagram), Some("sessionid=ig"));
        assert_eq!(jar.get(Platform::TikTok), None);
        assert_eq!(header_value(jar.get(Platform::YouTube).expect("yt"), "PREF"), Some("f6=40000000"));
    }

    #[test]
    fn debug_output_hides_values() {
        let jar = PlatformCookies::from_cookies(&parse_netscape(SAMPLE, 1_800_000_000).expect("parsed"));
        let shown = format!("{jar:?} {:?}", parse_netscape(SAMPLE, 0).expect("parsed"));
        assert!(!shown.contains("abc/def") && !shown.contains("login") && !shown.contains("ig\""), "{shown}");
        assert!(shown.contains("YouTube"));
    }

    #[test]
    fn host_matching() {
        let c = |domain: &str, sub| Cookie {
            domain: domain.into(),
            include_subdomains: sub,
            path: "/".into(),
            secure: true,
            expires: 0,
            name: "a".into(),
            value: "b".into(),
        };
        assert!(c("youtube.com", true).matches_host("www.youtube.com"));
        assert!(!c("youtube.com", false).matches_host("www.youtube.com"));
        assert!(!c("tube.com", true).matches_host("www.youtube.com"));
        assert!(c("www.youtube.com", false).matches_host("WWW.YouTube.com"));
    }
}

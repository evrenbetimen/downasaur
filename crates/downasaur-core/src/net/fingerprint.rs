//! Coherent browser header profiles.
//!
//! Servers cross-check the `User-Agent` against client hints and `Accept-*`
//! headers, so a profile carries a full, internally consistent header set
//! rather than a bare UA string.

use reqwest::RequestBuilder;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Desktop,
    /// Mobile web; several platforms serve simpler, CDN-direct payloads to phones.
    Mobile,
}

#[derive(Debug, Clone, Copy)]
pub struct BrowserProfile {
    pub user_agent: &'static str,
    pub sec_ch_ua: Option<&'static str>,
    pub sec_ch_ua_platform: Option<&'static str>,
    pub mobile: bool,
}

const DESKTOP: &[BrowserProfile] = &[
    BrowserProfile {
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
        sec_ch_ua: Some(r#""Chromium";v="141", "Google Chrome";v="141", "Not?A_Brand";v="99""#),
        sec_ch_ua_platform: Some(r#""Windows""#),
        mobile: false,
    },
    BrowserProfile {
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
        sec_ch_ua: None,
        sec_ch_ua_platform: None,
        mobile: false,
    },
    BrowserProfile {
        user_agent: "Mozilla/5.0 (X11; Linux x86_64; rv:143.0) Gecko/20100101 Firefox/143.0",
        sec_ch_ua: None,
        sec_ch_ua_platform: None,
        mobile: false,
    },
];

const MOBILE: &[BrowserProfile] = &[
    BrowserProfile {
        user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1",
        sec_ch_ua: None,
        sec_ch_ua_platform: None,
        mobile: true,
    },
    BrowserProfile {
        user_agent: "Mozilla/5.0 (Linux; Android 16; Pixel 10) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36",
        sec_ch_ua: Some(r#""Chromium";v="141", "Google Chrome";v="141", "Not?A_Brand";v="99""#),
        sec_ch_ua_platform: Some(r#""Android""#),
        mobile: true,
    },
];

impl BrowserProfile {
    pub fn all(device: DeviceClass) -> &'static [Self] {
        match device {
            DeviceClass::Desktop => DESKTOP,
            DeviceClass::Mobile => MOBILE,
        }
    }

    pub fn random(device: DeviceClass) -> Self {
        let pool = Self::all(device);
        pool[fastrand::usize(..pool.len())]
    }

    pub fn apply(&self, mut req: RequestBuilder) -> RequestBuilder {
        req = req
            .header(USER_AGENT, self.user_agent)
            .header(ACCEPT, "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8")
            .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9");
        if let Some(v) = self.sec_ch_ua {
            req = req.header("sec-ch-ua", v).header("sec-ch-ua-mobile", if self.mobile { "?1" } else { "?0" });
        }
        if let Some(v) = self.sec_ch_ua_platform {
            req = req.header("sec-ch-ua-platform", v);
        }
        req
    }
}

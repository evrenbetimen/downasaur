//! User-configured proxies (HTTP, HTTPS, SOCKS5).

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    /// `http://host:port`, `https://host:port` or `socks5h://host:port`.
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    pub fn to_reqwest(&self) -> Result<reqwest::Proxy> {
        let mut p = reqwest::Proxy::all(&self.url)
            .map_err(|e| CoreError::Parse(format!("invalid proxy `{}`: {e}", self.url)))?;
        if let (Some(u), Some(pw)) = (&self.username, &self.password) {
            p = p.basic_auth(u, pw);
        }
        Ok(p)
    }
}

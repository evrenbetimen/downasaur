//! HTTP layer: browser-profile headers, user-agent rotation, proxy rotation and
//! polite retry/backoff for `403`/`429`/`5xx` responses.
//!
//! [`HttpPool`] keeps one `reqwest::Client` per configured proxy (reqwest binds a
//! proxy at client build time) and hands them out round-robin. Each request picks
//! a [`fingerprint::BrowserProfile`] so header order and values stay consistent
//! with the advertised user agent.

pub mod fingerprint;
pub mod proxy;
pub mod retry;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use reqwest::{Client, RequestBuilder, Response, StatusCode};
use url::Url;

use crate::error::{CoreError, Result};
use fingerprint::{BrowserProfile, DeviceClass};
use proxy::ProxyConfig;
use retry::RetryPolicy;

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub proxies: Vec<ProxyConfig>,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub retry: RetryPolicy,
    /// Upper bound on idle pooled connections per host; 8K segment downloads open
    /// several ranged connections to the same CDN in parallel.
    pub pool_max_idle_per_host: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            proxies: Vec::new(),
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(60),
            retry: RetryPolicy::default(),
            pool_max_idle_per_host: 16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpPool {
    inner: Arc<PoolInner>,
}

#[derive(Debug)]
struct PoolInner {
    clients: Vec<Client>,
    cursor: AtomicUsize,
    retry: RetryPolicy,
}

impl HttpPool {
    pub fn new(config: HttpConfig) -> Result<Self> {
        let build = |proxy: Option<&ProxyConfig>| -> Result<Client> {
            let mut b = Client::builder()
                .cookie_store(true)
                .gzip(true)
                .brotli(true)
                .zstd(true)
                .connect_timeout(config.connect_timeout)
                .read_timeout(config.read_timeout)
                .pool_max_idle_per_host(config.pool_max_idle_per_host)
                .tcp_nodelay(true);
            if let Some(p) = proxy {
                b = b.proxy(p.to_reqwest()?);
            }
            Ok(b.build()?)
        };

        let clients = if config.proxies.is_empty() {
            vec![build(None)?]
        } else {
            config.proxies.iter().map(|p| build(Some(p))).collect::<Result<_>>()?
        };

        Ok(Self { inner: Arc::new(PoolInner { clients, cursor: AtomicUsize::new(0), retry: config.retry }) })
    }

    /// Next client in round-robin order (rotates proxies when several exist).
    pub fn client(&self) -> &Client {
        let i = self.inner.cursor.fetch_add(1, Ordering::Relaxed) % self.inner.clients.len();
        &self.inner.clients[i]
    }

    /// Build a GET with a randomly chosen browser profile of the given class.
    pub fn get(&self, url: &Url, device: DeviceClass) -> RequestBuilder {
        BrowserProfile::random(device).apply(self.client().get(url.clone()))
    }

    /// Send a request, retrying transient failures with jittered exponential backoff.
    ///
    /// `make` is invoked once per attempt so every retry can rotate client, proxy
    /// and user agent.
    pub async fn send_with_retry<F>(&self, mut make: F) -> Result<Response>
    where
        F: FnMut(&Self) -> RequestBuilder,
    {
        let policy = &self.inner.retry;
        let mut attempt = 0u32;
        loop {
            let outcome = make(self).send().await;
            let err = match outcome {
                Ok(resp) if resp.status().is_success() || resp.status() == StatusCode::PARTIAL_CONTENT => {
                    return Ok(resp);
                }
                Ok(resp) => status_error(&resp),
                Err(e) => CoreError::Network(e),
            };

            if attempt >= policy.max_retries || !policy.should_retry(&err) {
                return Err(err);
            }
            let delay = policy.delay_for(attempt, &err);
            tracing::debug!(attempt, ?delay, error = %err, "retrying request");
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    /// GET a URL and return the body as text, with retries.
    pub async fn get_text(&self, url: &Url, device: DeviceClass) -> Result<String> {
        let resp = self.send_with_retry(|pool| pool.get(url, device)).await?;
        Ok(resp.text().await?)
    }
}

fn status_error(resp: &Response) -> CoreError {
    match resp.status() {
        StatusCode::TOO_MANY_REQUESTS => CoreError::RateLimited {
            retry_after_secs: resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok()),
        },
        s => CoreError::AccessDenied { status: s.as_u16() },
    }
}

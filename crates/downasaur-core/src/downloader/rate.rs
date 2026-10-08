//! Token-bucket bandwidth limiter.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

#[derive(Debug, Clone, Default)]
pub struct RateLimiter {
    inner: Arc<Mutex<Option<Bucket>>>,
}

#[derive(Debug)]
struct Bucket {
    rate: f64,
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    /// `None` means unlimited.
    pub fn new(bytes_per_sec: Option<u64>) -> Self {
        let l = Self::default();
        l.set_limit(bytes_per_sec);
        l
    }

    pub fn set_limit(&self, bytes_per_sec: Option<u64>) {
        *self.inner.lock() = bytes_per_sec.map(|r| Bucket { rate: r as f64, tokens: r as f64, last: Instant::now() });
    }

    /// Reserve `n` bytes; returns how long the caller should wait first.
    pub fn reserve(&self, n: usize) -> Duration {
        let mut guard = self.inner.lock();
        let Some(b) = guard.as_mut() else { return Duration::ZERO };
        let now = Instant::now();
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * b.rate).min(b.rate);
        b.last = now;
        b.tokens -= n as f64;
        if b.tokens >= 0.0 { Duration::ZERO } else { Duration::from_secs_f64(-b.tokens / b.rate) }
    }

    pub async fn acquire(&self, n: usize) {
        let wait = self.reserve(n);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits() {
        assert_eq!(RateLimiter::new(None).reserve(1 << 30), Duration::ZERO);
    }

    #[test]
    fn overdraft_waits_proportionally() {
        let l = RateLimiter::new(Some(1000));
        assert_eq!(l.reserve(1000), Duration::ZERO);
        let w = l.reserve(500);
        assert!(w >= Duration::from_millis(450) && w <= Duration::from_millis(550), "{w:?}");
    }
}

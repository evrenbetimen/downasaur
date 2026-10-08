//! Retry policy with jittered exponential backoff that honours `Retry-After`.

use std::time::Duration;

use crate::error::CoreError;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// Retry `403` once or more: many CDNs return it for an expired or
    /// throttled signed URL and accept the same request from a fresh profile.
    pub retry_forbidden: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 4,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            retry_forbidden: true,
        }
    }
}

impl RetryPolicy {
    pub fn should_retry(&self, err: &CoreError) -> bool {
        match err {
            CoreError::AccessDenied { status: 403 } => self.retry_forbidden,
            CoreError::AccessDenied { status } => *status >= 500,
            other => other.is_retryable(),
        }
    }

    pub fn delay_for(&self, attempt: u32, err: &CoreError) -> Duration {
        if let CoreError::RateLimited { retry_after_secs: Some(s) } = err {
            return Duration::from_secs(*s).min(self.max_delay);
        }
        let exp = self.base_delay.saturating_mul(2u32.saturating_pow(attempt));
        let jitter = Duration::from_millis(fastrand::u64(0..=250));
        (exp + jitter).min(self.max_delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honours_retry_after() {
        let p = RetryPolicy::default();
        let d = p.delay_for(0, &CoreError::RateLimited { retry_after_secs: Some(7) });
        assert_eq!(d, Duration::from_secs(7));
    }

    #[test]
    fn backoff_is_capped() {
        let p = RetryPolicy::default();
        let d = p.delay_for(20, &CoreError::AccessDenied { status: 503 });
        assert!(d <= p.max_delay);
        assert!(p.should_retry(&CoreError::AccessDenied { status: 503 }));
        assert!(!p.should_retry(&CoreError::AccessDenied { status: 404 }));
    }
}

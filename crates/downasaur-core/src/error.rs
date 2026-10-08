//! Error type shared by every engine stage.
//!
//! Errors are serialized to the UI as `{ "code": "...", "message": "..." }` so the
//! frontend can branch on a stable `code` while showing a human-friendly message.

use serde::{Serialize, Serializer, ser::SerializeStruct};

pub type Result<T, E = CoreError> = std::result::Result<T, E>;

/// DRM schemes the engine can recognize. Detection only: protected content is
/// reported to the user and never downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DrmScheme {
    Widevine,
    FairPlay,
    PlayReady,
    Unknown,
}

impl std::fmt::Display for DrmScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Widevine => "Widevine",
            Self::FairPlay => "FairPlay",
            Self::PlayReady => "PlayReady",
            Self::Unknown => "an unknown DRM system",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("DRM Protected Content: this media is protected by {0} and cannot be downloaded.")]
    DrmProtected(DrmScheme),

    #[error("No extractor supports this URL: {0}")]
    UnsupportedUrl(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("Extraction failed on {platform}: {reason}")]
    Extraction { platform: &'static str, reason: String },

    #[error("The requested format is not available: {0}")]
    FormatUnavailable(String),

    #[error("Rate limited by the server (HTTP 429). Retry after {retry_after_secs:?}s.")]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("Access denied by the server (HTTP {status}).")]
    AccessDenied { status: u16 },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("FFmpeg failed: {0}")]
    Ffmpeg(String),

    #[error("External tool `{0}` was not found on PATH.")]
    ToolMissing(&'static str),

    #[error("Malformed data: {0}")]
    Parse(String),

    #[error("Task was cancelled.")]
    Cancelled,
}

impl CoreError {
    /// Stable machine-readable code for the UI.
    pub fn code(&self) -> &'static str {
        match self {
            Self::DrmProtected(_) => "drm_protected",
            Self::UnsupportedUrl(_) => "unsupported_url",
            Self::InvalidUrl(_) => "invalid_url",
            Self::Extraction { .. } => "extraction_failed",
            Self::FormatUnavailable(_) => "format_unavailable",
            Self::RateLimited { .. } => "rate_limited",
            Self::AccessDenied { .. } => "access_denied",
            Self::Network(_) => "network",
            Self::Io(_) => "io",
            Self::Database(_) => "database",
            Self::Ffmpeg(_) => "ffmpeg",
            Self::ToolMissing(_) => "tool_missing",
            Self::Parse(_) => "parse",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn extraction(platform: &'static str, reason: impl Into<String>) -> Self {
        Self::Extraction { platform, reason: reason.into() }
    }

    /// Whether retrying the same request later could plausibly succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. } => true,
            Self::Network(e) => e.is_timeout() || e.is_connect() || e.is_request(),
            _ => false,
        }
    }
}

impl Serialize for CoreError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("CoreError", 2)?;
        s.serialize_field("code", self.code())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drm_error_is_user_friendly_and_coded() {
        let err = CoreError::DrmProtected(DrmScheme::Widevine);
        assert_eq!(err.code(), "drm_protected");
        assert!(err.to_string().starts_with("DRM Protected Content"));
        let json = serde_json::to_value(&err).expect("serializes");
        assert_eq!(json["code"], "drm_protected");
    }
}

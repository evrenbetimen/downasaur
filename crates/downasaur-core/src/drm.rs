//! DRM safety catch.
//!
//! Downasaur never attempts to circumvent DRM. These helpers inspect manifests and
//! player metadata *before* any media bytes are fetched, and turn a positive match
//! into [`CoreError::DrmProtected`], which the UI renders as a friendly
//! "DRM Protected Content" notice instead of a crash or a corrupt file.

use crate::error::{CoreError, DrmScheme, Result};

/// DASH `ContentProtection@schemeIdUri` system IDs (lower-case, no braces).
const WIDEVINE_SYSTEM_ID: &str = "edef8ba9-79d6-4ace-a3c8-27dcd51d21ed";
const PLAYREADY_SYSTEM_ID: &str = "9a04f079-9840-4286-ab92-e65be0885f95";
const FAIRPLAY_SYSTEM_ID: &str = "94ce86fb-07ff-4f43-adb8-93d2fa968ca2";

/// HLS `EXT-X-KEY` / `EXT-X-SESSION-KEY` `KEYFORMAT` values.
const FAIRPLAY_KEYFORMAT: &str = "com.apple.streamingkeydelivery";
const PLAYREADY_KEYFORMAT: &str = "com.microsoft.playready";

/// Classify a DRM system identifier (UUID or `urn:uuid:` URI).
pub fn scheme_from_system_id(id: &str) -> Option<DrmScheme> {
    let id = id.trim().to_ascii_lowercase();
    let id = id.strip_prefix("urn:uuid:").unwrap_or(&id);
    match id {
        WIDEVINE_SYSTEM_ID => Some(DrmScheme::Widevine),
        PLAYREADY_SYSTEM_ID => Some(DrmScheme::PlayReady),
        FAIRPLAY_SYSTEM_ID => Some(DrmScheme::FairPlay),
        _ => None,
    }
}

/// Inspect raw HLS playlist text for DRM key systems.
///
/// Plain `METHOD=AES-128` (clear-key segment encryption with a fetchable key) is
/// *not* DRM and is handled by the regular HLS pipeline.
pub fn detect_in_hls(playlist: &str) -> Option<DrmScheme> {
    playlist.lines().filter(|l| l.starts_with("#EXT-X-KEY") || l.starts_with("#EXT-X-SESSION-KEY")).find_map(|line| {
        let upper = line.to_ascii_uppercase();
        if upper.contains("METHOD=NONE") || upper.contains("METHOD=AES-128") {
            return None;
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains(FAIRPLAY_KEYFORMAT) || lower.contains("skd://") {
            Some(DrmScheme::FairPlay)
        } else if lower.contains(PLAYREADY_KEYFORMAT) {
            Some(DrmScheme::PlayReady)
        } else if lower.contains(&format!("urn:uuid:{WIDEVINE_SYSTEM_ID}")) {
            Some(DrmScheme::Widevine)
        } else {
            Some(DrmScheme::Unknown)
        }
    })
}

/// Inspect a DASH MPD document for `ContentProtection` elements.
pub fn detect_in_mpd(mpd: &str) -> Option<DrmScheme> {
    let lower = mpd.to_ascii_lowercase();
    if !lower.contains("contentprotection") {
        return None;
    }
    [WIDEVINE_SYSTEM_ID, PLAYREADY_SYSTEM_ID, FAIRPLAY_SYSTEM_ID]
        .into_iter()
        .find(|id| lower.contains(id))
        .and_then(scheme_from_system_id)
        .or_else(|| {
            // `mp4protection:2011` alone means CENC without a recognized key system.
            lower.contains("urn:mpeg:dash:mp4protection:2011").then_some(DrmScheme::Unknown)
        })
}

/// Convenience: return `Err(DrmProtected)` if a scheme was detected.
pub fn ensure_clear(detected: Option<DrmScheme>) -> Result<()> {
    match detected {
        Some(scheme) => Err(CoreError::DrmProtected(scheme)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_aes128_hls_is_not_drm() {
        let pl = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://k/1\"\n#EXTINF:4,\na.ts\n";
        assert_eq!(detect_in_hls(pl), None);
    }

    #[test]
    fn fairplay_hls_is_detected() {
        let pl =
            "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://abc\",KEYFORMAT=\"com.apple.streamingkeydelivery\"\n";
        assert_eq!(detect_in_hls(pl), Some(DrmScheme::FairPlay));
        assert!(matches!(ensure_clear(detect_in_hls(pl)), Err(CoreError::DrmProtected(DrmScheme::FairPlay))));
    }

    #[test]
    fn widevine_mpd_is_detected() {
        let mpd = r#"<MPD><Period><AdaptationSet>
            <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc"/>
            <ContentProtection schemeIdUri="urn:uuid:EDEF8BA9-79D6-4ACE-A3C8-27DCD51D21ED"/>
        </AdaptationSet></Period></MPD>"#;
        assert_eq!(detect_in_mpd(mpd), Some(DrmScheme::Widevine));
    }

    #[test]
    fn plain_mpd_is_clear() {
        assert_eq!(detect_in_mpd("<MPD><Period/></MPD>"), None);
    }
}

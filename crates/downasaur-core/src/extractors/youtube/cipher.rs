//! Signature cipher support.
//!
//! Some formats ship a `signatureCipher` query string (`s`, `sp`, `url`) instead
//! of a playable URL. The player script transforms `s` into the real signature;
//! that transform is evaluated by [`super::jsc`], this module only unpacks and
//! re-assembles the URL.

use url::Url;

use crate::error::{CoreError, Result};

const NAME: &str = "YouTube";

/// Parsed `signatureCipher` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureCipher {
    pub url: Url,
    pub s: String,
    /// Query parameter that receives the deciphered signature (default `signature`).
    pub sp: String,
}

impl SignatureCipher {
    pub fn parse(raw: &str) -> Result<Self> {
        let (mut url, mut s, mut sp) = (None, None, None);
        for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
            match k.as_ref() {
                "url" => url = Some(v.into_owned()),
                "s" => s = Some(v.into_owned()),
                "sp" => sp = Some(v.into_owned()),
                _ => {}
            }
        }
        Ok(Self {
            url: Url::parse(&url.ok_or_else(|| CoreError::extraction(NAME, "cipher url missing"))?)?,
            s: s.ok_or_else(|| CoreError::extraction(NAME, "cipher signature missing"))?,
            sp: sp.unwrap_or_else(|| "signature".into()),
        })
    }

    /// The playable URL, given the deciphered signature for [`Self::s`].
    pub fn resolve(&self, signature: &str) -> Url {
        let mut url = self.url.clone();
        url.query_pairs_mut().append_pair(&self.sp, signature);
        url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_cipher_url() {
        let raw = "s=abcdef&sp=sig&url=https%3A%2F%2Frr.googlevideo.com%2Fvideoplayback%3Fitag%3D571";
        let c = SignatureCipher::parse(raw).expect("cipher");
        assert_eq!(c.s, "abcdef");
        let url = c.resolve("fedcba");
        assert_eq!(url.as_str(), "https://rr.googlevideo.com/videoplayback?itag=571&sig=fedcba");
    }

    #[test]
    fn defaults_signature_param() {
        let c = SignatureCipher::parse("s=x&url=https%3A%2F%2Fa.googlevideo.com%2Fv").expect("cipher");
        assert_eq!(c.sp, "signature");
    }
}

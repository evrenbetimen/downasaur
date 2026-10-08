//! Signature cipher support.
//!
//! Some formats ship a `signatureCipher` query string (`s`, `sp`, `url`) instead
//! of a playable URL. The player script defines a short transform program built
//! from three primitives (reverse, splice, swap) that maps `s` to the real
//! signature. This module locates that program in the player JS and evaluates it.

use regex::Regex;
use url::Url;

use crate::error::{CoreError, Result};

const NAME: &str = "YouTube";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherOp {
    Reverse,
    /// Drop the first `n` characters.
    Splice(usize),
    /// Swap character 0 with character `n % len`.
    Swap(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CipherProgram {
    pub ops: Vec<CipherOp>,
}

impl CipherProgram {
    pub fn apply(&self, sig: &str) -> String {
        let mut chars: Vec<char> = sig.chars().collect();
        for op in &self.ops {
            match *op {
                CipherOp::Reverse => chars.reverse(),
                CipherOp::Splice(n) => {
                    chars.drain(..n.min(chars.len()));
                }
                CipherOp::Swap(n) if !chars.is_empty() => {
                    let j = n % chars.len();
                    chars.swap(0, j);
                }
                CipherOp::Swap(_) => {}
            }
        }
        chars.into_iter().collect()
    }

    /// Locate the decipher function and its helper object in the player script.
    ///
    /// The decipher function looks like
    /// `xy=function(a){a=a.split("");Ab.cd(a,3);Ab.ef(a,49);return a.join("")}`
    /// and `Ab` is an object literal whose members wrap `reverse`, `splice` or a swap.
    pub fn from_player_js(js: &str) -> Result<Self> {
        let err = |what: &str| CoreError::extraction(NAME, format!("cipher {what} not found in player script"));

        let fn_re = Regex::new(
            r#"[A-Za-z0-9_$]+=function\(([A-Za-z0-9_$]+)\)\{\s*[A-Za-z0-9_$]+=[A-Za-z0-9_$]+\.split\(""\);(?P<body>[^}]+?)return [A-Za-z0-9_$]+\.join\(""\)\}"#,
        )
        .map_err(|e| CoreError::Parse(e.to_string()))?;
        let body = fn_re.captures(js).and_then(|c| c.name("body")).ok_or_else(|| err("function"))?.as_str();

        let call_re = Regex::new(r#"([A-Za-z0-9_$]+)\.([A-Za-z0-9_$]+)\([A-Za-z0-9_$]+,(\d+)\)"#)
            .map_err(|e| CoreError::Parse(e.to_string()))?;
        let calls: Vec<(String, String, usize)> = call_re
            .captures_iter(body)
            .filter_map(|c| Some((c[1].to_owned(), c[2].to_owned(), c[3].parse().ok()?)))
            .collect();
        let helper = calls.first().map(|c| c.0.clone()).ok_or_else(|| err("helper calls"))?;

        let obj_start = js.find(&format!("var {helper}={{")).ok_or_else(|| err("helper object"))?;
        let obj = &js[obj_start..js[obj_start..].find("};").map_or(js.len(), |e| obj_start + e)];

        let member_kind = |name: &str| -> Option<fn(usize) -> CipherOp> {
            let def_start = obj.find(&format!("{name}:function"))?;
            let def = &obj[def_start..];
            let def = &def[..def.find('}').unwrap_or(def.len())];
            if def.contains(".reverse(") {
                Some(|_| CipherOp::Reverse)
            } else if def.contains(".splice(") {
                Some(CipherOp::Splice)
            } else {
                Some(CipherOp::Swap)
            }
        };

        let ops = calls
            .iter()
            .map(|(_, member, arg)| member_kind(member).map(|k| k(*arg)).ok_or_else(|| err("helper member")))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { ops })
    }
}

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

    pub fn resolve(&self, program: &CipherProgram) -> Result<Url> {
        let mut url = self.url.clone();
        url.query_pairs_mut().append_pair(&self.sp, &program.apply(&self.s));
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER: &str = r#"
        var Xy={aB:function(a){a.reverse()},cD:function(a,b){a.splice(0,b)},
        eF:function(a,b){var c=a[0];a[0]=a[b%a.length];a[b%a.length]=c}};
        Qz=function(a){a=a.split("");Xy.eF(a,2);Xy.aB(a,0);Xy.cD(a,1);return a.join("")};
    "#;

    #[test]
    fn parses_and_applies_program() {
        let p = CipherProgram::from_player_js(PLAYER).expect("program");
        assert_eq!(p.ops, vec![CipherOp::Swap(2), CipherOp::Reverse, CipherOp::Splice(1)]);
        // "abcdef" -swap(2)-> "cbadef" -reverse-> "fedabc" -splice(1)-> "edabc"
        assert_eq!(p.apply("abcdef"), "edabc");
    }

    #[test]
    fn resolves_cipher_url() {
        let raw = "s=abcdef&sp=sig&url=https%3A%2F%2Frr.googlevideo.com%2Fvideoplayback%3Fitag%3D571";
        let c = SignatureCipher::parse(raw).expect("cipher");
        let program = CipherProgram { ops: vec![CipherOp::Reverse] };
        let url = c.resolve(&program).expect("url");
        assert_eq!(url.as_str(), "https://rr.googlevideo.com/videoplayback?itag=571&sig=fedcba");
    }
}

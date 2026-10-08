//! Clean, OS-safe file naming.
//!
//! [`TitleCleaner`] is a trait so a model-backed rewriter can be plugged in
//! later; [`HeuristicCleaner`] is the default, offline, deterministic one. It
//! strips emojis, hashtags, clickbait phrases, bracketed noise and tracking
//! tokens, transliterates to ASCII, and joins words with underscores:
//! `"🔥 iPhone 18 Review: SHOCKING!!! #apple"` by `MKBHD` → `MKBHD_iPhone_18_Review`.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

pub trait TitleCleaner: Send + Sync + std::fmt::Debug {
    /// Return a filename stem (no extension) for the given title and author.
    fn clean(&self, title: &str, author: Option<&str>) -> String;
}

#[derive(Debug, Clone)]
pub struct HeuristicCleaner {
    pub prefix_author: bool,
    pub max_len: usize,
}

impl Default for HeuristicCleaner {
    fn default() -> Self {
        Self { prefix_author: true, max_len: 120 }
    }
}

static HASHTAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)[#＃][\p{L}\p{N}_]+").expect("valid regex"));
static URLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").expect("valid regex"));
static TRACKING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:utm_[a-z]+|si|fbclid|igshid|feature)=[\w\-]+").expect("valid regex"));
static TRAILING_HASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[\s_\-|]+[0-9a-f]{8,}\s*$").expect("valid regex"));
static BRACKETED_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[\(\[【]\s*(?:official\s*(?:music\s*)?video|official|4k|8k|hd|uhd|hdr|full\s*video|lyrics?|free\s*download|new|must\s*watch)\s*[\)\]】]")
        .expect("valid regex")
});
static CLICKBAIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:you\s+won'?t\s+believe|gone\s+wrong|must\s+watch|not\s+clickbait|shocking|insane|unbelievable|omg|wtf)\b")
        .expect("valid regex")
});
static REPEATED_PUNCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[!?]{2,}").expect("valid regex"));

/// Windows device names that cannot be used as a file stem.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn is_emoji_or_symbol(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xFE00..=0xFE0F | 0x200D | 0x20E3 | 0xE0020..=0xE007F)
}

/// Split into words, fix SHOUTING, and join with `_`.
fn words(s: &str) -> Vec<String> {
    let ascii = deunicode::deunicode(s);
    let raw: Vec<&str> =
        ascii.split(|c: char| !(c.is_ascii_alphanumeric() || c == '\'')).filter(|w| !w.is_empty()).collect();
    let letters: Vec<char> = raw.iter().flat_map(|w| w.chars()).filter(char::is_ascii_alphabetic).collect();
    let mostly_caps =
        letters.len() > 6 && letters.iter().filter(|c| c.is_ascii_uppercase()).count() * 10 >= letters.len() * 7;
    raw.into_iter()
        .map(|w| w.replace('\'', ""))
        .filter(|w| !w.is_empty())
        .map(|w| {
            let shouting = w.len() > 4 && w.chars().all(|c| !c.is_ascii_lowercase());
            if shouting || (mostly_caps && w.len() > 1) {
                let lower = w.to_ascii_lowercase();
                let mut c = lower.chars();
                c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
            } else {
                w
            }
        })
        .collect()
}

impl TitleCleaner for HeuristicCleaner {
    fn clean(&self, title: &str, author: Option<&str>) -> String {
        let mut t: String = title.nfkc().filter(|c| !is_emoji_or_symbol(*c)).collect();
        for re in [&*URLS, &*TRACKING, &*HASHTAGS, &*BRACKETED_NOISE, &*CLICKBAIT, &*REPEATED_PUNCT] {
            t = re.replace_all(&t, " ").into_owned();
        }
        t = TRAILING_HASH.replace(&t, "").into_owned();

        let mut parts = words(&t);
        if self.prefix_author
            && let Some(a) = author
        {
            let author_words: Vec<String> = deunicode::deunicode(a)
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect();
            let joined = author_words.join("_");
            let already =
                parts.iter().take(author_words.len()).map(|w| w.to_ascii_lowercase()).collect::<Vec<_>>().join("_")
                    == joined.to_ascii_lowercase();
            if !joined.is_empty() && !already {
                parts.insert(0, joined);
            }
        }

        let mut stem = String::new();
        for w in parts {
            if !stem.is_empty() && stem.len() + 1 + w.len() > self.max_len {
                break;
            }
            if !stem.is_empty() {
                stem.push('_');
            }
            stem.push_str(&w);
        }
        stem.truncate(self.max_len);
        let stem = if stem.is_empty() { "video".to_owned() } else { stem };
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&stem)) { format!("{stem}_") } else { stem }
    }
}

/// Make a single path component safe on Windows, macOS and Linux. Never returns
/// `.`/`..` and never contains a separator.
pub fn sanitize_component(s: &str) -> String {
    let cleaned: String =
        s.nfkc()
            .filter(|c| !is_emoji_or_symbol(*c))
            .map(|c| {
                if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    let mut out: String = trimmed.chars().take(80).collect();
    if out.chars().all(|c| c == '_' || c == '.') {
        out.clear();
    }
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&out)) {
        out.push('_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(t: &str, a: Option<&str>) -> String {
        HeuristicCleaner::default().clean(t, a)
    }

    #[test]
    fn cleans_clickbait_title() {
        assert_eq!(clean("🔥 iPhone 18 Review: SHOCKING!!! #apple #tech", Some("MKBHD")), "MKBHD_iPhone_18_Review");
    }

    #[test]
    fn strips_tracking_and_trailing_hashes() {
        assert_eq!(
            clean("Sunset timelapse (Official Video) utm_source=share - 9f8e7d6c5b4a", None),
            "Sunset_timelapse"
        );
    }

    #[test]
    fn transliterates_and_avoids_duplicate_author() {
        assert_eq!(clean("Café Tour in Zürich", Some("café")), "Cafe_Tour_in_Zurich");
    }

    #[test]
    fn tames_all_caps() {
        assert_eq!(clean("THE BEST CAMERA PHONE", None), "The_Best_Camera_Phone");
    }

    #[test]
    fn handles_reserved_and_empty() {
        assert_eq!(clean("CON", None), "CON_");
        assert_eq!(clean("🔥🔥🔥", None), "video");
        assert_eq!(sanitize_component(".."), "");
        assert_eq!(sanitize_component("a/b:c"), "a_b_c");
    }
}

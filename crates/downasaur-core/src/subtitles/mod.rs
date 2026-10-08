//! Subtitle selection, download and WebVTT → SRT conversion.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::SubtitleTrack;
use crate::net::HttpPool;
use crate::net::fingerprint::DeviceClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubtitleMode {
    #[default]
    Off,
    /// Mux as soft subtitle streams into the output container.
    Embed,
    /// Save next to the video as `.srt` files.
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitlePrefs {
    pub mode: SubtitleMode,
    /// Preferred languages in order (`["en", "es"]`). Empty means all.
    pub languages: Vec<String>,
    pub include_auto_generated: bool,
}

impl Default for SubtitlePrefs {
    fn default() -> Self {
        Self { mode: SubtitleMode::Off, languages: vec!["en".into()], include_auto_generated: true }
    }
}

/// Choose tracks per preferred language, preferring human-made captions.
pub fn pick_tracks<'a>(tracks: &'a [SubtitleTrack], prefs: &SubtitlePrefs) -> Vec<&'a SubtitleTrack> {
    if prefs.mode == SubtitleMode::Off {
        return Vec::new();
    }
    let eligible = |t: &&SubtitleTrack| prefs.include_auto_generated || !t.auto_generated;
    if prefs.languages.is_empty() {
        return tracks.iter().filter(eligible).collect();
    }
    prefs
        .languages
        .iter()
        .filter_map(|lang| {
            let base = |t: &&SubtitleTrack| t.language.split('-').next() == Some(lang.as_str()) || t.language == *lang;
            tracks.iter().filter(eligible).filter(base).min_by_key(|t| t.auto_generated)
        })
        .collect()
}

/// Convert WebVTT to SubRip. Drops the header, NOTE/STYLE blocks and cue
/// settings, and rewrites `.` millisecond separators to `,`.
pub fn vtt_to_srt(vtt: &str) -> String {
    let mut out = String::with_capacity(vtt.len());
    let mut index = 0;
    for block in vtt.replace("\r\n", "\n").split("\n\n") {
        let mut lines = block.lines().skip_while(|l| l.trim().is_empty()).peekable();
        let Some(first) = lines.peek().copied() else { continue };
        if first.starts_with("WEBVTT") || first.starts_with("NOTE") || first.starts_with("STYLE") {
            continue;
        }
        if !first.contains("-->") {
            lines.next(); // cue identifier
        }
        let Some(timing) = lines.next().filter(|l| l.contains("-->")) else { continue };
        let mut parts = timing.split("-->");
        let (Some(start), Some(rest)) = (parts.next(), parts.next()) else { continue };
        let end = rest.split_whitespace().next().unwrap_or("");
        let text: Vec<String> = lines.map(strip_tags).filter(|l| !l.trim().is_empty()).collect();
        if text.is_empty() {
            continue;
        }
        index += 1;
        out.push_str(&format!("{index}\n{} --> {}\n{}\n\n", srt_time(start.trim()), srt_time(end), text.join("\n")));
    }
    out
}

fn srt_time(t: &str) -> String {
    let t = t.replace('.', ",");
    // VTT allows `mm:ss.ttt`; SRT needs `hh:mm:ss,ttt`.
    if t.matches(':').count() == 1 { format!("00:{t}") } else { t }
}

fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut depth = 0;
    for c in line.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Download a track and write it to `dest` as SRT.
pub async fn fetch_as_srt(http: &HttpPool, track: &SubtitleTrack, dest: &Path) -> Result<()> {
    let body = http.get_text(&track.url, DeviceClass::Desktop).await?;
    let srt = if track.format == "srt" { body } else { vtt_to_srt(&body) };
    tokio::fs::write(dest, srt).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn converts_vtt() {
        let vtt = "WEBVTT\nKind: captions\n\n1\n00:00:01.000 --> 00:00:02.500 align:start\n<c>Hello</c> world\n\n00:03.000 --> 00:04.000\nBye\n";
        assert_eq!(
            vtt_to_srt(vtt),
            "1\n00:00:01,000 --> 00:00:02,500\nHello world\n\n2\n00:00:03,000 --> 00:00:04,000\nBye\n\n"
        );
    }

    #[test]
    fn prefers_manual_tracks() {
        let t = |lang: &str, auto| SubtitleTrack {
            language: lang.into(),
            label: lang.into(),
            url: Url::parse("https://x/s.vtt").expect("url"),
            format: "vtt".into(),
            auto_generated: auto,
        };
        let tracks = vec![t("en", true), t("en-US", false), t("fr", false)];
        let prefs = SubtitlePrefs { mode: SubtitleMode::Embed, ..Default::default() };
        let picked = pick_tracks(&tracks, &prefs);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].language, "en-US");
    }
}

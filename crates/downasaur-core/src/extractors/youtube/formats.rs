//! `streamingData` → [`StreamFormat`] mapping.

use serde_json::Value;
use url::Url;

use super::cipher::SignatureCipher;
use super::jsc::PlayerScript;
use crate::error::{CoreError, Result};
use crate::extractors::util::{at, at_str, at_u64};
use crate::model::{AudioCodec, MediaInfo, StreamFormat, StreamKind, Transport, VideoCodec};

/// Parse `mimeType` like `video/webm; codecs="vp09.00.51.08"` into (major, container, codecs).
fn split_mime(mime: &str) -> (&str, &str, &str) {
    let (ty, rest) = mime.split_once(';').unwrap_or((mime, ""));
    let (major, container) = ty.trim().split_once('/').unwrap_or((ty, ""));
    let codecs = rest.split_once("codecs=").map(|(_, c)| c.trim().trim_matches('"')).unwrap_or("");
    (major, container, codecs)
}

/// Map a single format object. `url` must already be resolved.
fn map_format(f: &Value, url: Url) -> Option<StreamFormat> {
    let (major, container, codecs) = split_mime(at_str(f, "mimeType")?);
    let kind = match (major, codecs.contains(',')) {
        (_, true) => StreamKind::Muxed,
        ("audio", _) => StreamKind::AudioOnly,
        _ => StreamKind::VideoOnly,
    };
    let first_codec = codecs.split(',').next().unwrap_or("").trim();
    let video_codec = (kind != StreamKind::AudioOnly).then(|| VideoCodec::from_codecs_attr(first_codec));
    let audio_codec = match kind {
        StreamKind::AudioOnly => Some(AudioCodec::from_codecs_attr(first_codec)),
        StreamKind::Muxed => codecs.split(',').nth(1).map(AudioCodec::from_codecs_attr),
        StreamKind::VideoOnly => None,
    };
    let hdr =
        at_str(f, "colorInfo/transferCharacteristics").is_some_and(|t| t.contains("SMPTEST2084") || t.contains("HLG"));

    Some(StreamFormat {
        id: at_u64(f, "itag")?.to_string(),
        url,
        kind,
        transport: Transport::Http { content_length: at_u64(f, "contentLength") },
        container: container.to_owned(),
        width: at_u64(f, "width").and_then(|v| u32::try_from(v).ok()),
        height: at_u64(f, "height").and_then(|v| u32::try_from(v).ok()),
        fps: at_u64(f, "fps").map(|v| v as f32),
        video_codec,
        audio_codec,
        bitrate: at_u64(f, "averageBitrate").or_else(|| at_u64(f, "bitrate")),
        hdr,
        headers: Vec::new(),
    })
}

fn all_formats(player: &Value) -> impl Iterator<Item = &Value> {
    ["streamingData/formats", "streamingData/adaptiveFormats"]
        .into_iter()
        .filter_map(move |p| at(player, p).and_then(Value::as_array))
        .flatten()
}

/// Map every format that has a direct URL. Ciphered formats are added later by
/// [`solve_challenges`].
pub fn parse_streaming_data(player: &Value) -> Vec<StreamFormat> {
    all_formats(player)
        .filter_map(|f| {
            let url = Url::parse(at_str(f, "url")?).ok()?;
            map_format(f, url)
        })
        .collect()
}

/// Rewrite every format for a JS-dependent client: decipher `signatureCipher`
/// formats and replace each URL's `n` parameter with the player's transform of it.
///
/// Without the `n` rewrite googlevideo throttles the transfer to about real-time
/// speed, which makes 8K downloads take longer than the video itself.
pub async fn solve_challenges(info: &mut MediaInfo, player: &Value, script: &PlayerScript) -> Result<()> {
    let ciphered: Vec<(&Value, SignatureCipher)> = all_formats(player)
        .filter_map(|f| Some((f, SignatureCipher::parse(at_str(f, "signatureCipher")?))))
        .map(|(f, c)| c.map(|c| (f, c)))
        .collect::<Result<_>>()?;

    let mut n_values: Vec<String> = info
        .formats
        .iter()
        .map(|f| &f.url)
        .chain(ciphered.iter().map(|(_, c)| &c.url))
        .filter_map(|u| query_value(u, "n"))
        .collect();
    n_values.sort();
    n_values.dedup();
    let sig_values: Vec<String> = ciphered.iter().map(|(_, c)| c.s.clone()).collect();

    let solved = script.solve(n_values, sig_values).await?;

    for (f, c) in &ciphered {
        let sig = solved.sig.get(&c.s).ok_or_else(|| CoreError::extraction("YouTube", "signature not solved"))?;
        if let Some(fmt) = map_format(f, c.resolve(sig)) {
            info.formats.push(fmt);
        }
    }
    for fmt in &mut info.formats {
        if let Some(n) = query_value(&fmt.url, "n") {
            let solved_n =
                solved.n.get(&n).ok_or_else(|| CoreError::extraction("YouTube", "n parameter not solved"))?;
            set_query_value(&mut fmt.url, "n", solved_n);
        }
    }
    Ok(())
}

fn query_value(url: &Url, key: &str) -> Option<String> {
    url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

/// Replace `key`'s value in place, keeping parameter order.
fn set_query_value(url: &mut Url, key: &str, value: &str) {
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let v = if k == key { value.to_owned() } else { v.into_owned() };
            (k.into_owned(), v)
        })
        .collect();
    url.query_pairs_mut().clear().extend_pairs(pairs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_mime() {
        assert_eq!(split_mime(r#"video/webm; codecs="vp9""#), ("video", "webm", "vp9"));
        assert_eq!(
            split_mime(r#"video/mp4; codecs="avc1.42001E, mp4a.40.2""#),
            ("video", "mp4", "avc1.42001E, mp4a.40.2")
        );
    }

    #[test]
    fn replaces_n_in_place() {
        let mut u = Url::parse("https://rr.googlevideo.com/videoplayback?itag=571&n=abc&sig=x%2By").expect("url");
        assert_eq!(query_value(&u, "n").as_deref(), Some("abc"));
        set_query_value(&mut u, "n", "xyz");
        assert_eq!(u.as_str(), "https://rr.googlevideo.com/videoplayback?itag=571&n=xyz&sig=x%2By");
    }
}

//! `streamingData` → [`StreamFormat`] mapping.

use serde_json::Value;
use url::Url;

use super::cipher::{self, CipherProgram};
use crate::error::{CoreError, Result};
use crate::extractors::ExtractContext;
use crate::extractors::util::{at, at_str, at_u64, json_string_field};
use crate::model::{AudioCodec, MediaInfo, StreamFormat, StreamKind, Transport, VideoCodec};
use crate::net::fingerprint::DeviceClass;

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
/// [`resolve_ciphers`].
pub fn parse_streaming_data(player: &Value) -> Vec<StreamFormat> {
    all_formats(player)
        .filter_map(|f| {
            let url = Url::parse(at_str(f, "url")?).ok()?;
            map_format(f, url)
        })
        .collect()
}

/// Decipher `signatureCipher` formats using the transform program from the
/// current player script and append them to `info.formats`.
pub async fn resolve_ciphers(ctx: &ExtractContext, info: &mut MediaInfo, player: &Value) -> Result<()> {
    let ciphered: Vec<&Value> = all_formats(player).filter(|f| f.get("signatureCipher").is_some()).collect();
    if ciphered.is_empty() {
        return Ok(());
    }

    let program = load_cipher_program(ctx, &info.id).await?;
    for f in ciphered {
        let Some(raw) = at_str(f, "signatureCipher") else { continue };
        let parts = cipher::SignatureCipher::parse(raw)?;
        let url = parts.resolve(&program)?;
        if let Some(fmt) = map_format(f, url) {
            info.formats.push(fmt);
        }
    }
    Ok(())
}

async fn load_cipher_program(ctx: &ExtractContext, video_id: &str) -> Result<CipherProgram> {
    let embed = Url::parse(&format!("https://www.youtube.com/embed/{video_id}"))?;
    let html = ctx.http.get_text(&embed, DeviceClass::Desktop).await?;
    let js_path = json_string_field(&html, "jsUrl")
        .ok_or_else(|| CoreError::extraction("YouTube", "player script URL not found"))?;
    let js_url = Url::parse("https://www.youtube.com")?.join(&js_path)?;
    let js = ctx.http.get_text(&js_url, DeviceClass::Desktop).await?;
    CipherProgram::from_player_js(&js)
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
}

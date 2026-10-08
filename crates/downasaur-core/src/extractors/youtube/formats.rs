//! `streamingData` → [`StreamFormat`] mapping.

use serde_json::Value;
use url::Url;

use super::cipher::SignatureCipher;
use super::jsc::PlayerScript;
use crate::error::{CoreError, Result};
use crate::extractors::util::{at, at_str, at_u64};
use crate::model::{AudioCodec, MediaInfo, StreamFormat, StreamKind, Transport, VideoCodec};

/// Live HLS audio renditions name their group after the itag.
fn hls_audio_bitrate(group: &str) -> Option<u64> {
    match group {
        "233" => Some(48_000),
        "234" => Some(128_000),
        _ => None,
    }
}

/// Formats of a live stream's HLS master playlist (`hlsManifestUrl`).
///
/// Video variants that reference an `AUDIO` group carry no sound of their own,
/// so they become [`StreamKind::VideoOnly`] and each audio rendition becomes an
/// [`StreamKind::AudioOnly`] format; the remux stage joins the two recordings.
pub fn parse_hls_master(text: &str, base: &Url) -> Result<Vec<StreamFormat>> {
    let master = m3u8_rs::parse_master_playlist_res(text.as_bytes())
        .map_err(|e| CoreError::Parse(format!("invalid HLS master playlist: {e}")))?;
    let hls = |id: String, url: Url, kind| StreamFormat {
        id,
        url,
        kind,
        transport: Transport::Hls { live: true },
        container: "ts".into(),
        width: None,
        height: None,
        fps: None,
        video_codec: None,
        audio_codec: None,
        bitrate: None,
        hdr: false,
        headers: Vec::new(),
    };

    let mut formats: Vec<StreamFormat> = master
        .variants
        .iter()
        .filter(|v| !v.is_i_frame)
        .filter_map(|v| {
            let url = base.join(&v.uri).ok()?;
            let codecs: Vec<&str> = v.codecs.as_deref().unwrap_or("").split(',').map(str::trim).collect();
            let video = codecs.iter().find(|c| !c.starts_with("mp4a") && !c.starts_with("opus"))?;
            let height = v.resolution.and_then(|r| u32::try_from(r.height).ok());
            let kind = if v.audio.is_some() { StreamKind::VideoOnly } else { StreamKind::Muxed };
            let mut f = hls(format!("hls-{}p", height.unwrap_or(0)), url, kind);
            f.width = v.resolution.and_then(|r| u32::try_from(r.width).ok());
            f.height = height;
            f.fps = v.frame_rate.map(|r| r as f32);
            f.video_codec = Some(VideoCodec::from_codecs_attr(video));
            f.audio_codec = (kind == StreamKind::Muxed)
                .then(|| codecs.iter().find(|c| c.starts_with("mp4a") || c.starts_with("opus")))
                .flatten()
                .map(|c| AudioCodec::from_codecs_attr(c));
            f.bitrate = v.average_bandwidth.or(Some(v.bandwidth));
            Some(f)
        })
        .collect();

    for alt in master.alternatives.iter().filter(|a| a.media_type == m3u8_rs::AlternativeMediaType::Audio) {
        let Some(url) = alt.uri.as_deref().and_then(|u| base.join(u).ok()) else { continue };
        // The codec is only named on the variants that use this group.
        let codec = master
            .variants
            .iter()
            .filter(|v| v.audio.as_deref() == Some(alt.group_id.as_str()))
            .filter_map(|v| v.codecs.as_deref())
            .flat_map(|c| c.split(',').map(str::trim))
            .find(|c| c.starts_with("mp4a") || c.starts_with("opus"));
        let mut f = hls(format!("hls-audio-{}", alt.group_id), url, StreamKind::AudioOnly);
        f.audio_codec = Some(codec.map_or(AudioCodec::Aac, AudioCodec::from_codecs_attr));
        f.bitrate = hls_audio_bitrate(&alt.group_id);
        formats.push(f);
    }
    // One audio rendition per group is enough (groups differ by bitrate, not language).
    formats.dedup_by(|a, b| a.id == b.id);
    Ok(formats)
}

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
    fn parses_live_hls_master() {
        let master = "#EXTM3U\n\
            #EXT-X-MEDIA:URI=\"https://m.example/a233.m3u8\",TYPE=AUDIO,GROUP-ID=\"233\",NAME=\"Default\",DEFAULT=YES\n\
            #EXT-X-MEDIA:URI=\"https://m.example/a234.m3u8\",TYPE=AUDIO,GROUP-ID=\"234\",NAME=\"Default\",DEFAULT=YES\n\
            #EXT-X-STREAM-INF:BANDWIDTH=269034,CODECS=\"avc1.42C00B,mp4a.40.5\",RESOLUTION=256x144,FRAME-RATE=15,AUDIO=\"233\"\n\
            https://m.example/v144.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=4561186,CODECS=\"avc1.640028,mp4a.40.2\",RESOLUTION=1920x1080,FRAME-RATE=30,AUDIO=\"234\"\n\
            https://m.example/v1080.m3u8\n";
        let base = Url::parse("https://manifest.googlevideo.com/api/manifest/hls_variant/x").expect("url");
        let fs = parse_hls_master(master, &base).expect("parsed");
        let ids: Vec<_> = fs.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["hls-144p", "hls-1080p", "hls-audio-233", "hls-audio-234"]);
        let v = &fs[1];
        assert_eq!(
            (v.kind, v.height, v.video_codec, v.audio_codec),
            (StreamKind::VideoOnly, Some(1080), Some(VideoCodec::H264), None)
        );
        assert_eq!(v.transport, Transport::Hls { live: true });
        let a = &fs[3];
        assert_eq!((a.kind, a.audio_codec, a.bitrate), (StreamKind::AudioOnly, Some(AudioCodec::Aac), Some(128_000)));
        assert_eq!(a.url.as_str(), "https://m.example/a234.m3u8");
    }

    #[test]
    fn replaces_n_in_place() {
        let mut u = Url::parse("https://rr.googlevideo.com/videoplayback?itag=571&n=abc&sig=x%2By").expect("url");
        assert_eq!(query_value(&u, "n").as_deref(), Some("abc"));
        set_query_value(&mut u, "n", "xyz");
        assert_eq!(u.as_str(), "https://rr.googlevideo.com/videoplayback?itag=571&n=xyz&sig=x%2By");
    }
}

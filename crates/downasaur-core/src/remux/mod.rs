//! FFmpeg remux/transcode stage.
//!
//! Adaptive 8K downloads arrive as a separate video (AV1/VP9) and audio
//! (Opus/AAC) file. They are joined with stream copy (`-c copy`): no re-encode,
//! no quality loss, and the job is I/O-bound rather than CPU-bound. Progress is
//! read from `-progress pipe:1` and reported as a percentage of the duration.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::error::{CoreError, Result};
use crate::model::{AudioOutput, Container};

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleInput {
    pub path: PathBuf,
    /// ISO 639 language for the stream's metadata.
    pub language: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RemuxJob {
    /// Merge video + audio (+ soft subtitles) without re-encoding.
    Merge {
        video: PathBuf,
        audio: Option<PathBuf>,
        subtitles: Vec<SubtitleInput>,
        container: Container,
        output: PathBuf,
    },
    /// Re-wrap a single input (e.g. HLS `.ts` → `.mp4`).
    Rewrap { input: PathBuf, container: Container, output: PathBuf },
    /// Extract/transcode audio.
    Audio { input: PathBuf, format: AudioOutput, output: PathBuf },
}

impl RemuxJob {
    pub fn output(&self) -> &Path {
        match self {
            Self::Merge { output, .. } | Self::Rewrap { output, .. } | Self::Audio { output, .. } => output,
        }
    }

    /// Full FFmpeg argument list (without the binary).
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> =
            ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats"]
                .map(String::from)
                .to_vec();
        let path = |p: &Path| p.to_string_lossy().into_owned();

        match self {
            Self::Merge { video, audio, subtitles, container, output } => {
                a.extend(["-i".into(), path(video)]);
                if let Some(audio) = audio {
                    a.extend(["-i".into(), path(audio)]);
                }
                for s in subtitles {
                    a.extend(["-i".into(), path(&s.path)]);
                }
                a.extend(["-map".into(), "0:v:0".into()]);
                a.extend(["-map".into(), if audio.is_some() { "1:a:0".into() } else { "0:a?".into() }]);
                let sub_base = if audio.is_some() { 2 } else { 1 };
                for (i, s) in subtitles.iter().enumerate() {
                    a.extend(["-map".into(), format!("{}:s:0", sub_base + i)]);
                    a.extend([format!("-metadata:s:s:{i}"), format!("language={}", s.language)]);
                }
                a.extend(["-c:v".into(), "copy".into(), "-c:a".into(), "copy".into()]);
                if !subtitles.is_empty() {
                    let codec = match container {
                        Container::Mkv => "srt",
                        Container::Mp4 => "mov_text",
                    };
                    a.extend(["-c:s".into(), codec.into()]);
                }
                Self::container_flags(&mut a, *container);
                a.push(path(output));
            }
            Self::Rewrap { input, container, output } => {
                a.extend(["-i".into(), path(input), "-map".into(), "0".into(), "-c".into(), "copy".into()]);
                if *container == Container::Mp4 {
                    // ADTS AAC from MPEG-TS needs the bitstream filter to live in MP4.
                    a.extend(["-bsf:a".into(), "aac_adtstoasc".into()]);
                }
                Self::container_flags(&mut a, *container);
                a.push(path(output));
            }
            Self::Audio { input, format, output } => {
                a.extend(["-i".into(), path(input), "-vn".into()]);
                let codec: &[&str] = match format {
                    AudioOutput::Mp3 => &["-c:a", "libmp3lame", "-q:a", "0"],
                    AudioOutput::M4a => &["-c:a", "aac", "-b:a", "256k"],
                    AudioOutput::Opus => &["-c:a", "copy"],
                    AudioOutput::Flac => &["-c:a", "flac"],
                };
                a.extend(codec.iter().map(|s| (*s).to_owned()));
                a.push(path(output));
            }
        }
        a
    }

    fn container_flags(a: &mut Vec<String>, container: Container) {
        if container == Container::Mp4 {
            // Move the moov atom up front so huge 8K files start playing instantly.
            a.extend(["-movflags".into(), "+faststart".into()]);
        }
    }
}

/// Parse an `out_time_us=` / `out_time_ms=` progress line into seconds.
pub fn parse_progress_line(line: &str) -> Option<f64> {
    let (key, value) = line.split_once('=')?;
    match key {
        // Both keys are in microseconds (`out_time_ms` is misnamed upstream).
        "out_time_us" | "out_time_ms" => value.trim().parse::<f64>().ok().map(|us| us / 1_000_000.0),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub binary: PathBuf,
}

impl Default for Ffmpeg {
    fn default() -> Self {
        Self { binary: PathBuf::from("ffmpeg") }
    }
}

impl Ffmpeg {
    /// Run `job`, calling `on_progress(0.0..=100.0)` as FFmpeg advances.
    pub async fn run<F>(
        &self,
        job: &RemuxJob,
        duration_secs: Option<f64>,
        cancel: &CancellationToken,
        on_progress: F,
    ) -> Result<()>
    where
        F: Fn(f32),
    {
        let mut child = Command::new(&self.binary)
            .args(job.args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => CoreError::ToolMissing("ffmpeg"),
                _ => CoreError::Io(e),
            })?;

        let stdout = child.stdout.take().ok_or_else(|| CoreError::Ffmpeg("no stdout".into()))?;
        let mut lines = BufReader::new(stdout).lines();
        loop {
            let line = tokio::select! {
                () = cancel.cancelled() => {
                    child.start_kill()?;
                    return Err(CoreError::Cancelled);
                }
                line = lines.next_line() => line?,
            };
            let Some(line) = line else { break };
            if let (Some(t), Some(d)) = (parse_progress_line(&line), duration_secs.filter(|d| *d > 0.0)) {
                on_progress(((t / d) * 100.0).clamp(0.0, 100.0) as f32);
            }
        }

        let out = child.wait_with_output().await?;
        if out.status.success() {
            on_progress(100.0);
            Ok(())
        } else {
            Err(CoreError::Ffmpeg(String::from_utf8_lossy(&out.stderr).trim().to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_uses_stream_copy_and_maps_subtitles() {
        let job = RemuxJob::Merge {
            video: "v.webm".into(),
            audio: Some("a.webm".into()),
            subtitles: vec![SubtitleInput { path: "en.srt".into(), language: "eng".into() }],
            container: Container::Mkv,
            output: "out.mkv".into(),
        };
        let args = job.args().join(" ");
        assert!(args.contains("-map 0:v:0 -map 1:a:0 -map 2:s:0"));
        assert!(args.contains("-c:v copy -c:a copy -c:s srt"));
        assert!(args.ends_with("out.mkv"));
        assert!(!args.contains("faststart"));
    }

    #[test]
    fn mp4_gets_faststart() {
        let job = RemuxJob::Rewrap { input: "in.ts".into(), container: Container::Mp4, output: "o.mp4".into() };
        assert!(job.args().join(" ").contains("-movflags +faststart"));
    }

    #[test]
    fn parses_progress() {
        assert_eq!(parse_progress_line("out_time_us=1500000"), Some(1.5));
        assert_eq!(parse_progress_line("progress=continue"), None);
    }
}

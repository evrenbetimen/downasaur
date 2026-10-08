//! Run one URL through the whole engine: extract → download → remux → organize.
//!
//! ```text
//! cargo run --release -p downasaur-core --example fetch -- <url> [output-dir] [--probe] [--cookies=cookies.txt]
//! ```
//!
//! `--probe` only lists the formats. Set `RUST_LOG=downasaur_core=debug` for detail.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use downasaur_core::cookies::PlatformCookies;
use downasaur_core::engine::{Engine, EngineConfig};
use downasaur_core::model::{DownloadProfile, Extraction, TaskState};
use downasaur_core::organizer::OrganizerRules;
use downasaur_core::progress::ProgressEvent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let probe_only = args.iter().any(|a| a == "--probe");
    let cookies = args.iter().find_map(|a| a.strip_prefix("--cookies="));
    let mut positional = args.iter().filter(|a| !a.starts_with("--"));
    let url = positional.next().ok_or("usage: fetch <url> [output-dir] [--probe]")?;
    let out = PathBuf::from(positional.next().map_or("downasaur-out", String::as_str));

    let mut config = EngineConfig::with_data_dir(out.join(".downasaur"));
    if let Some(path) = cookies {
        config.cookies = PlatformCookies::load(Path::new(path))?;
        println!("cookies loaded for {:?}", config.cookies.platforms().map(|p| p.display_name()).collect::<Vec<_>>());
    }
    let engine = Engine::new(config)?;
    engine.set_organizer_rules(OrganizerRules { target_dir: out.clone(), ..Default::default() })?;

    let started = Instant::now();
    let info = match engine.probe(url).await? {
        Extraction::Media(info) => *info,
        Extraction::Collection { .. } => return Err("collections are not supported by this example".into()),
    };
    println!(
        "{} by {:?} ({} formats, probed in {:.1?})",
        info.title,
        info.author,
        info.formats.len(),
        started.elapsed()
    );
    let mut formats = info.formats.clone();
    formats.sort_by_key(|f| std::cmp::Reverse((f.height.unwrap_or(0), f.bitrate.unwrap_or(0))));
    for f in &formats {
        println!(
            "  itag {:>4} {:>5}p {:>3}fps {:<10} {:?}/{:?} {:>6} kbps",
            f.id,
            f.height.unwrap_or(0),
            f.fps.unwrap_or(0.0),
            f.container,
            f.video_codec,
            f.audio_codec,
            f.bitrate.unwrap_or(0) / 1000,
        );
    }
    if probe_only {
        return Ok(());
    }

    let task = engine.enqueue(&info, DownloadProfile::ultra_8k())?;
    let mut last_print = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        for ev in engine.hub().drain() {
            match ev {
                ProgressEvent::Download { bytes_done, bytes_total, speed_bps, .. }
                    if last_print.elapsed() > Duration::from_secs(2) =>
                {
                    last_print = Instant::now();
                    let total = bytes_total.unwrap_or(0).max(1);
                    println!(
                        "  {:.1}% {:.0}/{:.0} MB @ {:.1} MB/s",
                        bytes_done as f64 * 100.0 / total as f64,
                        bytes_done as f64 / 1e6,
                        total as f64 / 1e6,
                        speed_bps / 1e6
                    );
                }
                ProgressEvent::Muxing { percent, label, .. } => println!("  {label} {percent:.0}%"),
                ProgressEvent::State { state, message, .. } => {
                    println!("state: {state:?} {}", message.unwrap_or_default())
                }
                ProgressEvent::Organized { destination, .. } => println!("filed at {destination}"),
                ProgressEvent::Download { .. } => {}
            }
        }
        let t = engine.list()?.into_iter().find(|t| t.id == task.id).ok_or("task vanished")?;
        match t.state {
            TaskState::Completed => {
                println!("done in {:.1?}: {}", started.elapsed(), t.destination.unwrap_or_default());
                return Ok(());
            }
            TaskState::Failed | TaskState::Cancelled => {
                return Err(format!("{:?}: {}", t.state, t.error.unwrap_or_default()).into());
            }
            _ => {}
        }
    }
}

//! Download engine: queue orchestration across every pipeline stage.
//!
//! `enqueue` persists a task and spawns its pipeline:
//! schedule gate → extract → select formats → ranged download (resumable) →
//! subtitles → FFmpeg remux → organizer → completed. Pause and cancel share a
//! cancellation token per task; the intent decides the final state. Because
//! chunk progress is checkpointed to SQLite, a paused, crashed or killed task
//! resumes from the last flushed byte.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::cookies::PlatformCookies;
use crate::db::Database;
use crate::downloader::hls::HlsJob;
use crate::downloader::http::HttpJob;
use crate::downloader::rate::RateLimiter;
use crate::downloader::{ByteCounter, DownloadOptions, chunks, plan_chunks};
use crate::error::{CoreError, Result};
use crate::extractors::{ExtractContext, ExtractorRegistry};
use crate::model::{
    Container, DownloadProfile, DownloadTask, Extraction, MediaInfo, Platform, StreamFormat, TaskState, Transport,
};
use crate::net::{HttpConfig, HttpPool};
use crate::organizer::{OrganizeRequest, Organizer, OrganizerRules};
use crate::progress::{ProgressEvent, ProgressHub, SpeedMeter};
use crate::remux::{Ffmpeg, RemuxJob, SubtitleInput};
use crate::scheduler::SchedulerConfig;
use crate::scheduler::network::{self, Metered};
use crate::selection;
use crate::subtitles::{self, SubtitleMode, SubtitlePrefs};

/// Chunk indices for the audio stream start here so both streams share one table.
const AUDIO_CHUNK_BASE: u32 = 1_000_000;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub data_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub max_concurrent: usize,
    pub download: DownloadOptions,
    pub http: HttpConfig,
    pub ffmpeg: PathBuf,
    pub yt_dlp: PathBuf,
    /// Cookies the user imported (see [`crate::cookies`]); empty by default.
    pub cookies: PlatformCookies,
}

impl EngineConfig {
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        Self {
            temp_dir: data_dir.join("partial"),
            data_dir,
            max_concurrent: 4,
            download: DownloadOptions::default(),
            http: HttpConfig::default(),
            ffmpeg: PathBuf::from("ffmpeg"),
            yt_dlp: PathBuf::from("yt-dlp"),
            cookies: PlatformCookies::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopIntent {
    Pause,
    Cancel,
}

#[derive(Debug)]
struct Active {
    token: CancellationToken,
    intent: Mutex<Option<StopIntent>>,
}

#[derive(Debug, Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    registry: ExtractorRegistry,
    ctx: ExtractContext,
    db: Database,
    hub: ProgressHub,
    organizer: Organizer,
    rules: RwLock<OrganizerRules>,
    scheduler: RwLock<SchedulerConfig>,
    subtitles: RwLock<SubtitlePrefs>,
    limiter: RateLimiter,
    ffmpeg: Ffmpeg,
    options: DownloadOptions,
    temp_dir: PathBuf,
    slots: Arc<Semaphore>,
    active: Mutex<HashMap<Uuid, Arc<Active>>>,
    metered: RwLock<Metered>,
}

mod keys {
    pub const RULES: &str = "organizer.rules";
    pub const SCHEDULER: &str = "scheduler.config";
    pub const SUBTITLES: &str = "subtitles.prefs";
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.temp_dir)?;
        let db = Database::open(&config.data_dir.join("downasaur.db"))?;
        let http = HttpPool::new(config.http.clone())?;
        let mut ctx = ExtractContext::new(http);
        ctx.yt_dlp = config.yt_dlp.clone();
        ctx.cookies = config.cookies.clone();

        let scheduler: SchedulerConfig = db.get_setting(keys::SCHEDULER)?.unwrap_or_default();
        Ok(Self {
            inner: Arc::new(Inner {
                registry: ExtractorRegistry::default(),
                ctx,
                rules: RwLock::new(db.get_setting(keys::RULES)?.unwrap_or_default()),
                subtitles: RwLock::new(db.get_setting(keys::SUBTITLES)?.unwrap_or_default()),
                limiter: RateLimiter::new(scheduler.bandwidth_limit),
                scheduler: RwLock::new(scheduler),
                db,
                hub: ProgressHub::default(),
                organizer: Organizer::default(),
                ffmpeg: Ffmpeg { binary: config.ffmpeg },
                options: config.download,
                temp_dir: config.temp_dir,
                slots: Arc::new(Semaphore::new(config.max_concurrent.max(1))),
                active: Mutex::new(HashMap::new()),
                metered: RwLock::new(Metered::Unknown),
            }),
        })
    }

    pub fn hub(&self) -> &ProgressHub {
        &self.inner.hub
    }

    pub fn detect_platform(&self, input: &str) -> Result<Platform> {
        Ok(self.inner.registry.detect_platform(&ExtractorRegistry::parse_input(input)?))
    }

    pub async fn probe(&self, input: &str) -> Result<Extraction> {
        let url = ExtractorRegistry::parse_input(input)?;
        self.inner.registry.extract(&self.inner.ctx, &url).await
    }

    pub fn list(&self) -> Result<Vec<DownloadTask>> {
        self.inner.db.list_tasks()
    }

    // ---- settings -------------------------------------------------------

    pub fn organizer_rules(&self) -> OrganizerRules {
        self.inner.rules.read().clone()
    }

    pub fn set_organizer_rules(&self, rules: OrganizerRules) -> Result<()> {
        self.inner.db.set_setting(keys::RULES, &rules)?;
        *self.inner.rules.write() = rules;
        Ok(())
    }

    pub fn scheduler_config(&self) -> SchedulerConfig {
        self.inner.scheduler.read().clone()
    }

    pub fn set_scheduler_config(&self, cfg: SchedulerConfig) -> Result<()> {
        self.inner.db.set_setting(keys::SCHEDULER, &cfg)?;
        self.inner.limiter.set_limit(cfg.bandwidth_limit);
        *self.inner.scheduler.write() = cfg;
        Ok(())
    }

    pub fn subtitle_prefs(&self) -> SubtitlePrefs {
        self.inner.subtitles.read().clone()
    }

    pub fn set_subtitle_prefs(&self, prefs: SubtitlePrefs) -> Result<()> {
        self.inner.db.set_setting(keys::SUBTITLES, &prefs)?;
        *self.inner.subtitles.write() = prefs;
        Ok(())
    }

    // ---- queue control --------------------------------------------------

    pub fn enqueue(&self, info: &MediaInfo, profile: DownloadProfile) -> Result<DownloadTask> {
        let now = Utc::now();
        let task = DownloadTask {
            id: Uuid::new_v4(),
            source_url: info.source_url.clone(),
            platform: info.platform,
            title: info.title.clone(),
            author: info.author.clone(),
            profile,
            state: TaskState::Queued,
            bytes_total: None,
            bytes_done: 0,
            destination: None,
            error: None,
            created_at: now,
            updated_at: now,
        };
        self.inner.db.insert_task(&task)?;
        self.spawn(task.id);
        Ok(task)
    }

    pub fn pause(&self, id: Uuid) -> Result<()> {
        self.stop(id, StopIntent::Pause)
    }

    pub fn cancel(&self, id: Uuid) -> Result<()> {
        self.stop(id, StopIntent::Cancel)
    }

    pub fn resume(&self, id: Uuid) -> Result<()> {
        if !self.inner.active.lock().contains_key(&id) {
            self.spawn(id);
        }
        Ok(())
    }

    fn stop(&self, id: Uuid, intent: StopIntent) -> Result<()> {
        if let Some(a) = self.inner.active.lock().get(&id) {
            *a.intent.lock() = Some(intent);
            a.token.cancel();
            return Ok(());
        }
        // Not running: update persisted state directly.
        let state = match intent {
            StopIntent::Pause => TaskState::Paused,
            StopIntent::Cancel => TaskState::Cancelled,
        };
        self.set_state(id, state, None)
    }

    /// Re-spawn tasks that were running when the app last exited.
    pub fn restore(&self) -> Result<usize> {
        let tasks = self.inner.db.interrupted_tasks()?;
        for t in &tasks {
            self.spawn(t.id);
        }
        Ok(tasks.len())
    }

    /// Start the metered-network guard. Running downloads pause when the
    /// connection becomes metered and `pause_on_metered` is on.
    pub fn start_network_guard(&self, cancel: CancellationToken) {
        let mut rx = network::spawn_guard(Duration::from_secs(30), cancel.clone());
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    changed = rx.changed() => if changed.is_err() { break },
                }
                let state = *rx.borrow();
                *engine.inner.metered.write() = state;
                if state.should_pause() && engine.inner.scheduler.read().pause_on_metered {
                    let ids: Vec<Uuid> = engine.inner.active.lock().keys().copied().collect();
                    for id in ids {
                        tracing::info!(%id, "pausing on metered connection");
                        let _ = engine.pause(id);
                    }
                }
            }
        });
    }

    fn set_state(&self, id: Uuid, state: TaskState, message: Option<String>) -> Result<()> {
        self.inner.db.set_state(id, state, message.as_deref())?;
        self.inner.hub.publish(ProgressEvent::State { task_id: id, state, message });
        Ok(())
    }

    fn spawn(&self, id: Uuid) {
        let active = Arc::new(Active { token: CancellationToken::new(), intent: Mutex::new(None) });
        self.inner.active.lock().insert(id, active.clone());
        let engine = self.clone();
        tokio::spawn(async move {
            let result = engine.run(id, &active.token).await;
            engine.inner.active.lock().remove(&id);
            let intent = *active.intent.lock();
            let outcome = match result {
                Ok(()) => Ok(()),
                Err(CoreError::Cancelled) => match intent {
                    Some(StopIntent::Pause) => engine.set_state(id, TaskState::Paused, None),
                    _ => {
                        engine.cleanup_partials(id).await;
                        engine.set_state(id, TaskState::Cancelled, None)
                    }
                },
                Err(e) => {
                    tracing::warn!(%id, error = %e, "task failed");
                    engine.set_state(id, TaskState::Failed, Some(e.to_string()))
                }
            };
            if let Err(e) = outcome {
                tracing::error!(%id, error = %e, "could not persist task state");
            }
        });
    }

    async fn cleanup_partials(&self, id: Uuid) {
        let _ = self.inner.db.clear_chunks(id);
        if let Ok(mut dir) = tokio::fs::read_dir(&self.inner.temp_dir).await {
            let prefix = id.to_string();
            while let Ok(Some(entry)) = dir.next_entry().await {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
    }

    // ---- pipeline -------------------------------------------------------

    async fn run(&self, id: Uuid, cancel: &CancellationToken) -> Result<()> {
        let inner = &self.inner;
        let task = inner.db.get_task(id)?.ok_or_else(|| CoreError::Parse(format!("unknown task {id}")))?;
        if task.state.is_terminal() {
            return Ok(());
        }

        // Wait for a free slot, the schedule window and an unmetered network.
        self.set_state(id, TaskState::Queued, None)?;
        let _permit = tokio::select! {
            () = cancel.cancelled() => return Err(CoreError::Cancelled),
            p = inner.slots.clone().acquire_owned() => p.map_err(|_| CoreError::Cancelled)?,
        };
        loop {
            let sched = inner.scheduler.read().clone();
            let metered_block = sched.pause_on_metered && inner.metered.read().should_pause();
            if sched.allows_now() && !metered_block {
                break;
            }
            self.set_state(id, TaskState::Scheduled, None)?;
            tokio::select! {
                () = cancel.cancelled() => return Err(CoreError::Cancelled),
                () = tokio::time::sleep(Duration::from_secs(30)) => {}
            }
        }

        self.set_state(id, TaskState::Fetching, None)?;
        let info = match inner.registry.extract(&inner.ctx, &task.source_url).await? {
            Extraction::Media(m) => *m,
            Extraction::Collection { .. } => {
                return Err(CoreError::extraction("engine", "collections must be expanded before enqueueing"));
            }
        };
        let picked = selection::select(&info, &task.profile)?;

        let warning = (!info.warnings.is_empty()).then(|| info.warnings.join("; "));
        self.set_state(id, TaskState::Downloading, warning)?;
        let counter = ByteCounter::new(inner.db.load_chunks(id).map(|c| chunks::bytes_done(&c)).unwrap_or(0));
        let reporter = self.spawn_reporter(id, counter.clone(), cancel.child_token());

        // Live renditions are recorded side by side from one shared starting
        // segment; fetched one after the other, the audio would come from later.
        let live_start = Mutex::new(None);
        let video = async {
            match &picked.video {
                Some(f) => self.fetch_stream(id, f, "v", 0, &counter, cancel, &live_start).await.map(Some),
                None => Ok(None),
            }
        };
        let audio = async {
            match &picked.audio {
                Some(f) => {
                    self.fetch_stream(id, f, "a", AUDIO_CHUNK_BASE, &counter, cancel, &live_start).await.map(Some)
                }
                None => Ok(None),
            }
        };
        let is_live =
            |f: &Option<StreamFormat>| f.as_ref().is_some_and(|f| f.transport == Transport::Hls { live: true });
        let (video_path, audio_path) = if is_live(&picked.video) || is_live(&picked.audio) {
            tokio::try_join!(video, audio)?
        } else {
            let video = video.await?;
            (video, audio.await?)
        };
        reporter.cancel();

        let subs = self.fetch_subtitles(id, &info).await;

        // Remux / transcode.
        self.set_state(id, TaskState::Muxing, None)?;
        let tmp = |ext: &str| inner.temp_dir.join(format!("{id}.out.{ext}"));
        let (job, audio_only) = match (&task.profile, video_path, audio_path) {
            (DownloadProfile::AudioOnly { format }, _, Some(a)) => {
                let ext = match format {
                    crate::model::AudioOutput::Mp3 => "mp3",
                    crate::model::AudioOutput::M4a => "m4a",
                    crate::model::AudioOutput::Opus => "opus",
                    crate::model::AudioOutput::Flac => "flac",
                };
                (RemuxJob::Audio { input: a, format: *format, output: tmp(ext) }, true)
            }
            (profile, Some(v), audio) => {
                let container = match profile {
                    DownloadProfile::Video { container, .. } => *container,
                    _ => Container::Mp4,
                };
                let embed = if subs.mode == SubtitleMode::Embed { subs.files.clone() } else { Vec::new() };
                (
                    RemuxJob::Merge {
                        video: v,
                        audio,
                        subtitles: embed,
                        container,
                        output: tmp(container.extension()),
                    },
                    false,
                )
            }
            _ => return Err(CoreError::FormatUnavailable("nothing was downloaded".into())),
        };
        let label = if picked.video.as_ref().and_then(|v| v.height).is_some_and(|h| h >= 4320) {
            "Muxing 8K Video..."
        } else {
            "Muxing..."
        };
        inner
            .ffmpeg
            .run(&job, info.duration_secs, cancel, |percent| {
                inner.hub.publish(ProgressEvent::Muxing { task_id: id, percent, label: label.to_owned() });
            })
            .await?;

        // Organize.
        self.set_state(id, TaskState::Organizing, None)?;
        let rules = inner.rules.read().clone();
        let final_path = if rules.enabled {
            let req = OrganizeRequest {
                file: job.output().to_path_buf(),
                platform: info.platform,
                title: info.title.clone(),
                author: info.author.clone(),
                published_at: info.published_at,
                content_kind: info.content_kind,
                height: picked.video.as_ref().and_then(|v| v.height),
                audio_only,
            };
            inner.organizer.organize(&rules, &req).await?
        } else {
            job.output().to_path_buf()
        };
        if subs.mode == SubtitleMode::External {
            for s in &subs.files {
                let dest = final_path.with_extension(format!("{}.srt", s.language));
                let _ = tokio::fs::rename(&s.path, dest).await;
            }
        }

        let dest = final_path.to_string_lossy().into_owned();
        inner.db.set_destination(id, &dest)?;
        inner.hub.publish(ProgressEvent::Organized { task_id: id, destination: dest });
        self.cleanup_partials(id).await;
        self.set_state(id, TaskState::Completed, None)
    }

    fn spawn_reporter(&self, id: Uuid, counter: ByteCounter, stop: CancellationToken) -> CancellationToken {
        let inner = self.inner.clone();
        let handle = stop.clone();
        tokio::spawn(async move {
            let mut meter = SpeedMeter::new(counter.get());
            let mut persisted = 0u64;
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                tokio::select! {
                    () = stop.cancelled() => break,
                    _ = tick.tick() => {}
                }
                let done = counter.get();
                let total = inner.db.get_task(id).ok().flatten().and_then(|t| t.bytes_total);
                let speed = meter.sample(done);
                inner.hub.publish(ProgressEvent::Download {
                    task_id: id,
                    bytes_done: done,
                    bytes_total: total,
                    speed_bps: speed,
                    eta_secs: meter.eta(done, total),
                });
                if done.saturating_sub(persisted) > 8 * 1024 * 1024 {
                    let _ = inner.db.set_progress(id, done, None);
                    persisted = done;
                }
            }
        });
        handle
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_stream(
        &self,
        id: Uuid,
        format: &StreamFormat,
        tag: &str,
        chunk_base: u32,
        counter: &ByteCounter,
        cancel: &CancellationToken,
        live_start: &Mutex<Option<u64>>,
    ) -> Result<PathBuf> {
        let inner = &self.inner;
        let dest = inner.temp_dir.join(format!("{id}.{tag}.{}", format.container));
        match format.transport {
            Transport::Hls { .. } => {
                // HLS restarts from scratch: segment URLs are short-lived.
                let _ = tokio::fs::remove_file(&dest).await;
                HlsJob {
                    pool: &inner.ctx.http,
                    playlist_url: &format.url,
                    dest: &dest,
                    parallel_segments: inner.options.connections,
                    buffer_capacity: inner.options.buffer_capacity,
                    limiter: &inner.limiter,
                    counter,
                    cancel,
                    max_duration: inner.options.live_max_duration,
                    live_start: Some(live_start),
                }
                .run()
                .await?;
            }
            Transport::Dash => {
                return Err(CoreError::FormatUnavailable("DASH manifests are not supported yet".into()));
            }
            Transport::Http { content_length } => {
                self.fetch_http(id, format, &dest, content_length, chunk_base, counter, cancel).await?;
            }
        }
        Ok(dest)
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_http(
        &self,
        id: Uuid,
        format: &StreamFormat,
        dest: &Path,
        content_length: Option<u64>,
        chunk_base: u32,
        counter: &ByteCounter,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let inner = &self.inner;
        let job = HttpJob {
            pool: &inner.ctx.http,
            url: &format.url,
            headers: &format.headers,
            dest,
            options: &inner.options,
            limiter: &inner.limiter,
            counter,
            cancel,
        };
        let probe = match content_length {
            Some(len) => crate::downloader::http::Probe { content_length: Some(len), ranges: true },
            None => job.probe().await?,
        };
        let (Some(total), true) = (probe.content_length, probe.ranges) else {
            job.run_sequential().await?;
            return Ok(());
        };

        let stream_of = |c: &crate::downloader::Chunk| c.index / AUDIO_CHUNK_BASE;
        let existing: Vec<_> =
            inner.db.load_chunks(id)?.into_iter().filter(|c| stream_of(c) == chunk_base / AUDIO_CHUNK_BASE).collect();
        let chunks = if existing.last().map(|c| c.end + 1) == Some(total) {
            existing
        } else {
            let mut planned = plan_chunks(total, inner.options.chunk_size);
            for c in &mut planned {
                c.index += chunk_base;
            }
            inner.db.save_chunks(id, &planned)?;
            planned
        };
        // Task total = every planned chunk across video and audio streams.
        let all_bytes: u64 = inner.db.load_chunks(id)?.iter().map(crate::downloader::Chunk::len).sum();
        inner.db.set_progress(id, counter.get(), Some(all_bytes))?;

        let db = inner.db.clone();
        job.run(chunks, move |c| {
            if let Err(e) = db.update_chunk(id, &c) {
                tracing::warn!(%id, error = %e, "chunk checkpoint failed");
            }
        })
        .await?;
        Ok(())
    }

    async fn fetch_subtitles(&self, id: Uuid, info: &MediaInfo) -> FetchedSubs {
        let prefs = self.inner.subtitles.read().clone();
        let mut files = Vec::new();
        for track in subtitles::pick_tracks(&info.subtitles, &prefs) {
            let path = self.inner.temp_dir.join(format!("{id}.{}.srt", track.language));
            match subtitles::fetch_as_srt(&self.inner.ctx.http, track, &path).await {
                Ok(()) => files.push(SubtitleInput { path, language: track.language.clone() }),
                Err(e) => tracing::warn!(%id, lang = %track.language, error = %e, "subtitle fetch failed"),
            }
        }
        FetchedSubs { mode: prefs.mode, files }
    }

    /// Expand a collection URL into individual media for batch enqueueing.
    pub async fn expand(&self, entries: &[Url]) -> Vec<Result<MediaInfo>> {
        let mut out = Vec::with_capacity(entries.len());
        for url in entries {
            out.push(match self.inner.registry.extract(&self.inner.ctx, url).await {
                Ok(Extraction::Media(m)) => Ok(*m),
                Ok(Extraction::Collection { .. }) => {
                    Err(CoreError::extraction("engine", "nested collections are not expanded"))
                }
                Err(e) => Err(e),
            });
        }
        out
    }
}

#[derive(Debug)]
struct FetchedSubs {
    mode: SubtitleMode,
    files: Vec<SubtitleInput>,
}

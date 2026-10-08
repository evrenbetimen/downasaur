//! Smart auto-organizer.
//!
//! Completed files are moved to
//! `{target_dir}/{media class dir?}/{pattern}/{clean name}.{ext}` where the
//! pattern defaults to `{Platform}/{Author}/{Year}-{Month}` and the optional
//! media-class directory separates audio (`Music`), 8K video (`Videos/8K`) and
//! Shorts/Reels/Stories (`Shorts`). Work runs on a background task fed by a
//! channel so the download workers never block on filesystem moves.

pub mod naming;
pub mod pattern;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::error::Result;
use crate::model::{ContentKind, Platform, Resolution};
use naming::{HeuristicCleaner, TitleCleaner};
use pattern::{PathPattern, PatternContext};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSeparation {
    pub enabled: bool,
    pub audio_dir: String,
    pub eight_k_dir: String,
    pub shorts_dir: String,
}

impl Default for MediaSeparation {
    fn default() -> Self {
        Self { enabled: true, audio_dir: "Music".into(), eight_k_dir: "Videos/8K".into(), shorts_dir: "Shorts".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizerRules {
    pub enabled: bool,
    pub target_dir: PathBuf,
    /// Template string, e.g. `{Platform}/{Author}/{Year}-{Month}`.
    pub pattern: String,
    pub separation: MediaSeparation,
    pub clean_names: bool,
}

impl Default for OrganizerRules {
    fn default() -> Self {
        Self {
            enabled: true,
            target_dir: PathBuf::from("Downasaur"),
            pattern: PathPattern::default().to_template(),
            separation: MediaSeparation::default(),
            clean_names: true,
        }
    }
}

/// Metadata the organizer needs about a finished file.
#[derive(Debug, Clone, PartialEq)]
pub struct OrganizeRequest {
    pub file: PathBuf,
    pub platform: Platform,
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub content_kind: ContentKind,
    pub height: Option<u32>,
    pub audio_only: bool,
}

#[derive(Debug, Clone)]
pub struct Organizer {
    cleaner: Arc<dyn TitleCleaner>,
}

impl Default for Organizer {
    fn default() -> Self {
        Self { cleaner: Arc::new(HeuristicCleaner::default()) }
    }
}

impl Organizer {
    pub fn with_cleaner(cleaner: Arc<dyn TitleCleaner>) -> Self {
        Self { cleaner }
    }

    fn media_dir<'a>(rules: &'a OrganizerRules, req: &OrganizeRequest) -> Option<&'a str> {
        let sep = &rules.separation;
        if !sep.enabled {
            return None;
        }
        if req.audio_only {
            Some(&sep.audio_dir)
        } else if matches!(req.content_kind, ContentKind::Short | ContentKind::Story) {
            Some(&sep.shorts_dir)
        } else if req.height.is_some_and(|h| Resolution::from_height(h) == Resolution::P4320) {
            Some(&sep.eight_k_dir)
        } else {
            None
        }
    }

    /// Compute the final destination without touching the filesystem.
    pub fn plan(&self, rules: &OrganizerRules, req: &OrganizeRequest) -> PathBuf {
        let date = req.published_at.unwrap_or_else(Utc::now);
        let ctx = PatternContext {
            platform: req.platform.display_name().to_owned(),
            author: req.author.clone(),
            year: date.year(),
            month: date.month(),
            day: date.day(),
            resolution: req
                .height
                .map(|h| Resolution::from_height(h).label().split(' ').next().unwrap_or("").to_owned()),
            media_type: if req.audio_only { "Audio".into() } else { "Video".into() },
        };

        let mut dir = rules.target_dir.clone();
        if let Some(m) = Self::media_dir(rules, req) {
            dir.extend(m.split('/').map(naming::sanitize_component).filter(|c| !c.is_empty()));
        }
        dir.push(PathPattern::parse(&rules.pattern).render(&ctx));

        let ext = req.file.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
        let stem = if rules.clean_names {
            self.cleaner.clean(&req.title, req.author.as_deref())
        } else {
            naming::sanitize_component(&req.title)
        };
        dir.join(format!("{stem}.{ext}"))
    }

    /// Move the file into place, creating directories and avoiding collisions.
    pub async fn organize(&self, rules: &OrganizerRules, req: &OrganizeRequest) -> Result<PathBuf> {
        let planned = self.plan(rules, req);
        if let Some(parent) = planned.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let dest = unique_path(&planned).await;
        move_file(&req.file, &dest).await?;
        tracing::info!(from = %req.file.display(), to = %dest.display(), "organized");
        Ok(dest)
    }

    /// Spawn a background worker; send `(rules, request, reply)` jobs to it.
    pub fn spawn_worker(self) -> mpsc::Sender<OrganizeJob> {
        let (tx, mut rx) = mpsc::channel::<OrganizeJob>(256);
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let result = self.organize(&job.rules, &job.request).await;
                // The requester may have gone away (task cancelled); that's fine.
                let _ = job.reply.send(result);
            }
        });
        tx
    }
}

#[derive(Debug)]
pub struct OrganizeJob {
    pub rules: OrganizerRules,
    pub request: OrganizeRequest,
    pub reply: oneshot::Sender<Result<PathBuf>>,
}

/// `name.mp4` → `name (2).mp4` → `name (3).mp4` ... until free.
async fn unique_path(p: &Path) -> PathBuf {
    if !tokio::fs::try_exists(p).await.unwrap_or(false) {
        return p.to_path_buf();
    }
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|e| e.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    for n in 2.. {
        let candidate = p.with_file_name(format!("{stem} ({n}){ext}"));
        if !tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            return candidate;
        }
    }
    unreachable!("unbounded range always yields a free name")
}

/// Rename, falling back to copy + delete across filesystems.
async fn move_file(from: &Path, to: &Path) -> Result<()> {
    match tokio::fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(_) => {
            tokio::fs::copy(from, to).await?;
            tokio::fs::remove_file(from).await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn req(kind: ContentKind, height: Option<u32>, audio_only: bool) -> OrganizeRequest {
        OrganizeRequest {
            file: PathBuf::from("/tmp/dl/abc.mkv"),
            platform: Platform::YouTube,
            title: "iPhone 18 Review!!! 🔥".into(),
            author: Some("MKBHD".into()),
            published_at: Utc.with_ymd_and_hms(2027, 3, 9, 12, 0, 0).single(),
            content_kind: kind,
            height,
            audio_only,
        }
    }

    fn rules() -> OrganizerRules {
        OrganizerRules { target_dir: PathBuf::from("/media"), ..Default::default() }
    }

    #[test]
    fn plans_8k_into_its_own_tree() {
        let p = Organizer::default().plan(&rules(), &req(ContentKind::Video, Some(4320), false));
        assert_eq!(p, PathBuf::from("/media/Videos/8K/YouTube/MKBHD/2027-03/MKBHD_iPhone_18_Review.mkv"));
    }

    #[test]
    fn separates_shorts_and_audio() {
        let o = Organizer::default();
        assert!(o.plan(&rules(), &req(ContentKind::Short, Some(1920), false)).starts_with("/media/Shorts"));
        assert!(o.plan(&rules(), &req(ContentKind::Video, None, true)).starts_with("/media/Music"));
        let mut r = rules();
        r.separation.enabled = false;
        assert_eq!(
            o.plan(&r, &req(ContentKind::Video, Some(1080), false)),
            PathBuf::from("/media/YouTube/MKBHD/2027-03/MKBHD_iPhone_18_Review.mkv")
        );
    }

    #[tokio::test]
    async fn organize_moves_and_dedupes() {
        let root = std::env::temp_dir().join(format!("downasaur-org-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.expect("mkdir");
        let r = OrganizerRules { target_dir: root.join("out"), ..Default::default() };
        let o = Organizer::default();
        let mut dests = Vec::new();
        for i in 0..2 {
            let src = root.join(format!("in{i}.mp4"));
            tokio::fs::write(&src, b"x").await.expect("write");
            let mut rq = req(ContentKind::Video, Some(1080), false);
            rq.file = src;
            dests.push(o.organize(&r, &rq).await.expect("organized"));
        }
        assert_ne!(dests[0], dests[1]);
        assert!(dests[1].to_string_lossy().ends_with("MKBHD_iPhone_18_Review (2).mp4"));
        tokio::fs::remove_dir_all(&root).await.expect("cleanup");
    }
}

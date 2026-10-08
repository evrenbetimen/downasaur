//! Organizer, scheduler and subtitle settings.

use std::path::PathBuf;

use downasaur_core::CoreError;
use downasaur_core::model::{ContentKind, Platform};
use downasaur_core::organizer::{OrganizeRequest, Organizer, OrganizerRules};
use downasaur_core::scheduler::SchedulerConfig;
use downasaur_core::subtitles::SubtitlePrefs;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub fn get_organizer_rules(state: State<'_, AppState>) -> OrganizerRules {
    state.engine.organizer_rules()
}

#[tauri::command]
pub fn set_organizer_rules(state: State<'_, AppState>, rules: OrganizerRules) -> Result<(), CoreError> {
    state.engine.set_organizer_rules(rules)
}

/// Live preview for the rules dashboard using a representative sample video.
#[tauri::command]
pub fn preview_organizer_path(rules: OrganizerRules) -> String {
    let sample = OrganizeRequest {
        file: PathBuf::from("sample.mp4"),
        platform: Platform::YouTube,
        title: "🔥 iPhone 18 Review: SHOCKING!!! #apple".into(),
        author: Some("MKBHD".into()),
        published_at: None,
        content_kind: ContentKind::Video,
        height: Some(4320),
        audio_only: false,
    };
    Organizer::default().plan(&rules, &sample).to_string_lossy().into_owned()
}

#[tauri::command]
pub fn get_scheduler_config(state: State<'_, AppState>) -> SchedulerConfig {
    state.engine.scheduler_config()
}

#[tauri::command]
pub fn set_scheduler_config(state: State<'_, AppState>, config: SchedulerConfig) -> Result<(), CoreError> {
    state.engine.set_scheduler_config(config)
}

#[tauri::command]
pub fn get_subtitle_prefs(state: State<'_, AppState>) -> SubtitlePrefs {
    state.engine.subtitle_prefs()
}

#[tauri::command]
pub fn set_subtitle_prefs(state: State<'_, AppState>, prefs: SubtitlePrefs) -> Result<(), CoreError> {
    state.engine.set_subtitle_prefs(prefs)
}

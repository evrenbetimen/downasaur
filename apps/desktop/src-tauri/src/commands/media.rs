//! Link sniffing and metadata extraction.

use downasaur_core::CoreError;
use downasaur_core::model::{Extraction, MediaInfo, Platform};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SniffResult {
    pub platform: Platform,
    pub display_name: &'static str,
}

/// Offline platform detection; cheap enough to run on every clipboard change.
#[tauri::command]
pub fn sniff_link(state: State<'_, AppState>, input: String) -> Result<SniffResult, CoreError> {
    let platform = state.engine.detect_platform(&input)?;
    Ok(SniffResult { platform, display_name: platform.display_name() })
}

#[tauri::command]
pub async fn fetch_metadata(state: State<'_, AppState>, input: String) -> Result<Extraction, CoreError> {
    state.engine.probe(&input).await
}

/// Expand playlist/channel entries into media. Failed entries are skipped and
/// reported by URL so the UI can show them.
#[tauri::command]
pub async fn expand_collection(
    state: State<'_, AppState>,
    entries: Vec<url::Url>,
) -> Result<Vec<MediaInfo>, CoreError> {
    Ok(state.engine.expand(&entries).await.into_iter().filter_map(Result::ok).collect())
}

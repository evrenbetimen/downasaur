//! Download queue control.

use downasaur_core::CoreError;
use downasaur_core::model::{DownloadProfile, DownloadTask, MediaInfo};
use tauri::State;
use uuid::Uuid;

use crate::state::AppState;

/// Enqueue one or more items with the same profile (batch format picker).
#[tauri::command]
pub async fn enqueue_download(
    state: State<'_, AppState>,
    items: Vec<MediaInfo>,
    profile: DownloadProfile,
) -> Result<Vec<DownloadTask>, CoreError> {
    items.iter().map(|info| state.engine.enqueue(info, profile.clone())).collect()
}

#[tauri::command]
pub fn list_downloads(state: State<'_, AppState>) -> Result<Vec<DownloadTask>, CoreError> {
    state.engine.list()
}

#[tauri::command]
pub fn pause_download(state: State<'_, AppState>, id: Uuid) -> Result<(), CoreError> {
    state.engine.pause(id)
}

#[tauri::command]
pub async fn resume_download(state: State<'_, AppState>, id: Uuid) -> Result<(), CoreError> {
    state.engine.resume(id)
}

#[tauri::command]
pub fn cancel_download(state: State<'_, AppState>, id: Uuid) -> Result<(), CoreError> {
    state.engine.cancel(id)
}

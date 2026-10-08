//! Rust → UI event bridge.
//!
//! Progress events are coalesced by the engine's `ProgressHub` and flushed at
//! most once per frame (~16.7 ms, 60 fps) as a single batched payload, so the
//! webview's IPC channel stays cheap even with 100+ active downloads.

use tauri::{AppHandle, Emitter};

use crate::state::AppState;

pub const PROGRESS_EVENT: &str = "downasaur://progress";

pub fn start(app: AppHandle, state: &AppState) {
    let engine = state.engine.clone();
    let shutdown = state.shutdown.clone();
    tauri::async_runtime::spawn(async move {
        let emitter = app.clone();
        engine.hub().spawn_flusher(
            move |batch| {
                if let Err(e) = emitter.emit(PROGRESS_EVENT, &batch) {
                    tracing::warn!(error = %e, "failed to emit progress batch");
                }
            },
            shutdown.clone(),
        );
        engine.start_network_guard(shutdown);
        match engine.restore() {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "resumed interrupted downloads"),
            Err(e) => tracing::error!(error = %e, "could not restore queue"),
        }
    });
}

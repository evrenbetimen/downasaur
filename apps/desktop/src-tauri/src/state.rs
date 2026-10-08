//! Application state managed by Tauri.

use downasaur_core::engine::{Engine, EngineConfig};
use downasaur_core::organizer::OrganizerRules;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct AppState {
    pub engine: Engine,
    /// Cancelled on exit to stop background loops (progress flusher, network guard).
    pub shutdown: CancellationToken,
}

impl AppState {
    pub fn initialize(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = app.path().app_data_dir()?;
        let engine = Engine::new(EngineConfig::with_data_dir(data_dir))?;

        // First run: point the organizer at the user's Videos folder.
        let rules = engine.organizer_rules();
        if rules.target_dir.is_relative() {
            let base = directories::UserDirs::new()
                .and_then(|u| u.video_dir().map(std::path::Path::to_path_buf))
                .or_else(|| app.path().download_dir().ok());
            if let Some(base) = base {
                engine.set_organizer_rules(OrganizerRules { target_dir: base.join("Downasaur"), ..rules })?;
            }
        }

        Ok(Self { engine, shutdown: CancellationToken::new() })
    }
}

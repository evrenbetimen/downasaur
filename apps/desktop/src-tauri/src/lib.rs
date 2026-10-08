//! Tauri v2 shell for Downasaur 8K.
//!
//! Owns the [`Engine`](downasaur_core::engine::Engine), exposes it to the React
//! UI as typed commands (see [`commands`]), and streams progress back through a
//! single `downasaur://progress` event batched at 60 fps (see [`events`]).

mod commands;
mod events;
mod state;

use tauri::Manager;
use tracing_subscriber::EnvFilter;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,downasaur_core=debug")),
        )
        .init();

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = state::AppState::initialize(app.handle())?;
            events::start(app.handle().clone(), &state);
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::media::sniff_link,
            commands::media::fetch_metadata,
            commands::media::expand_collection,
            commands::queue::enqueue_download,
            commands::queue::list_downloads,
            commands::queue::pause_download,
            commands::queue::resume_download,
            commands::queue::cancel_download,
            commands::settings::get_organizer_rules,
            commands::settings::set_organizer_rules,
            commands::settings::preview_organizer_path,
            commands::settings::get_scheduler_config,
            commands::settings::set_scheduler_config,
            commands::settings::get_subtitle_prefs,
            commands::settings::set_subtitle_prefs,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = result {
        tracing::error!(error = %e, "Downasaur exited with an error");
        std::process::exit(1);
    }
}

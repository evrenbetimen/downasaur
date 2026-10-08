//! Tauri commands. Every command returns `Result<T, CoreError>`; `CoreError`
//! serializes to `{ code, message }` for the UI.

pub mod media;
pub mod queue;
pub mod settings;

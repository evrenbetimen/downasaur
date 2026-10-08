//! # downasaur-core
//!
//! The systems engine behind Downasaur 8K. It is UI-agnostic: the Tauri shell in
//! `apps/desktop/src-tauri` wraps it with commands and events, but every module
//! here can be driven (and tested) without a webview.
//!
//! Pipeline overview:
//!
//! ```text
//! URL ──► extractors::ExtractorRegistry ──► MediaInfo + formats
//!                                            │
//!                    downloader (ranged, resumable, buffered) ◄── db (WAL queue state)
//!                                            │
//!                       remux (FFmpeg stream copy, subtitles)
//!                                            │
//!                 organizer (rule-based paths + clean naming) ──► disk
//! ```
//!
//! Progress from every stage flows through [`progress::ProgressHub`], which
//! coalesces updates to at most one frame every ~16 ms (60 fps) per sink.

pub mod db;
pub mod downloader;
pub mod drm;
pub mod engine;
pub mod error;
pub mod extractors;
pub mod model;
pub mod net;
pub mod organizer;
pub mod progress;
pub mod remux;
pub mod scheduler;
pub mod selection;
pub mod subtitles;

pub use error::{CoreError, Result};

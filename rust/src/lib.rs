// Bongo Player: lightweight IPTV player on libmpv + egui

/// Product name as users see it: window title, --version, help overlay.
pub const APP_NAME: &str = "Bongo Player";
/// Technical name: exe, ini, data folder, log file and User-Agent.
pub const APP_ID: &str = "BongoPlayer";
/// This crate's tracing target (its crate name), for the default log filter.
pub const LOG_TARGET: &str = module_path!();

pub mod app;
pub mod args;
pub mod catalog;
pub mod epg;
pub mod favorites;
pub mod legacy;
pub mod playback_health;
pub mod player;
pub mod portal;
pub mod presets;
pub mod redact;
pub mod relay;
pub mod search;
pub mod shortcuts;
pub mod storage;
pub mod zap;

//! The guide's data, without drawing: time arithmetic, the EPG store, the
//! background fetcher, the cache file and the channel logos. `guide_ui.rs`
//! draws the guide from it; `app.rs` plays what the user picks.

pub mod cache;
pub mod fetch;
pub mod store;
pub mod time;

pub use crate::catalog::GuideChannel;

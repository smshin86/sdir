pub mod filesystem;
pub mod listing;
pub mod live_search;
pub mod model;
pub mod pane_arrangement;
pub mod platform;
pub mod search;
pub mod settings;
pub mod worker;

pub mod history;

pub mod tree;

pub mod watch;

pub mod file_appearance;
pub mod file_preview;
pub mod utility;

pub mod index_cache;
pub mod index_policy;
pub mod preferences;

pub mod batch;

pub mod journal;

pub mod drag;

pub mod open_with;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_spotlight;

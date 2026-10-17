//! koan-tui — Ratatui TUI for koan music player.
//!
//! Library crate that exports `run_tui()`. No main(), no clap, no CLI args.
//! Depends on koan-core only.

pub mod accent;
pub mod actions_menu;
pub mod album_page;
pub mod app;
pub mod artist_header;
pub mod browse;
pub mod browse_filter;
pub mod context_menu;
pub mod control;
pub mod cover_art;
pub mod device_selector;
pub mod downloads_page;
pub mod dsp_selector;
#[cfg(test)]
mod dumps;
pub mod enqueue;
pub mod help_modal;
pub mod history_page;
pub mod keys;
pub mod lyrics;
pub mod media_keys;
pub mod nav;
pub mod now_playing;
pub mod organize;
pub mod picker;
pub mod picker_items;
pub mod play;
pub mod playlist_modals;
pub mod playlist_ops;
pub mod playlists_page;
pub mod queue;
pub mod remote_bridge;
pub mod rows;
pub mod settings_page;
pub mod shelf_page;
pub mod shell;
pub mod tasks;
pub mod text;
pub mod theme;
pub mod thumb_cache;
pub mod track_info;
pub mod track_rows;
pub mod transport;
pub mod ui;
pub mod visualizer;
pub mod viz_picker;

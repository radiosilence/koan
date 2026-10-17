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
pub mod context_menu;
pub mod cover_art;
pub mod device_selector;
pub mod dsp_selector;
pub mod enqueue;
pub mod help_modal;
pub mod history_page;
pub mod keys;
pub mod library;
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
pub mod settings_page;
pub mod tasks;
pub mod theme;
pub mod track_details;
pub mod track_info;
pub mod transport;
pub mod ui;
pub mod visualizer;
pub mod viz_picker;

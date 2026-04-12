//! koan-tui — Ratatui TUI for koan music player.
//!
//! Library crate that exports `run_tui()`. No main(), no clap, no CLI args.
//! Depends on koan-core. Uses koan-server for the embedded API server.

pub mod app;
pub mod context_menu;
pub mod cover_art;
pub mod device_selector;
pub mod download_queue;
pub mod enqueue;
pub mod help_modal;
pub mod keys;
pub mod library;
pub mod lyrics;
pub mod media_keys;
pub mod organize;
pub mod picker;
pub mod picker_items;
pub mod play;
pub mod queue;
pub mod remote_bridge;
pub mod theme;
pub mod track_info;
pub mod transport;
pub mod ui;
pub mod visualizer;
pub mod viz_picker;

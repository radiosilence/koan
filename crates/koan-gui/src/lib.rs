//! The kōan GUI app, drawn with GPUI over koan-ffi. Desktop builds, macOS
//! and Linux, for now.

pub mod app;
pub mod collections;
pub mod components;
pub mod covers;
pub mod dsp;
pub mod engine;
pub mod library;
#[cfg(feature = "profiler")]
pub mod measure;
pub mod mirror;
pub mod navigator;
pub mod paint;
pub mod platform;
pub mod queue;
pub mod root;
pub mod session;
pub mod settings;
pub mod sheet;
pub mod shell;
pub mod sidebar;
pub mod theme;
pub mod time;
pub mod transport;
pub mod window_state;

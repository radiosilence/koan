//! kōan's design system for GPUI: the theme (tokens, type roles, Geist Mono,
//! the icon set, the accent, the wash) and the components every page is
//! built from. It knows nothing of the engine or of kōan's data, so it is
//! built and tested alone; `cargo run -p koan-kit` opens the storybook, which
//! shows every component.
//! The design is `docs/design/koan-kit.md`.

pub mod app;
pub mod components;
pub mod storybook;
pub mod theme;

/// GPUI past the components, for what koan-kit does not cover: each name a
/// page takes from here is counted by `just gui-theme-leaks`, with a comment
/// saying why.
pub mod raw {
    pub use crate::app::platform::{application, init};
    pub use gpui::*;
    pub use gpui_platform as platform;
}

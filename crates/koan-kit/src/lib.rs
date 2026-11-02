//! kōan's design system for GPUI: the theme (tokens, type roles, Geist Mono,
//! the icon set, the accent, the wash) and the components every page is
//! built from. It knows nothing of the engine or of kōan's data, so it is
//! built and tested alone; `examples/theme_sheet.rs` draws every component.
//! The design is `docs/design/koan-kit.md`.

// GPUI's test macro expands deeply around the menu's refill test.
#![recursion_limit = "512"]

pub mod app;
pub mod components;
pub mod sheet;
pub mod theme;

/// GPUI as koan-gui still reaches it directly, while its pages move onto
/// the components. It shrinks as they do, and goes when nothing uses it.
pub mod raw {
    pub use gpui_kit::*;
}

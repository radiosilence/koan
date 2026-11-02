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

/// GPUI as koan-gui still reaches it directly, while its pages move onto
/// the components. It shrinks as they do, and goes when nothing uses it.
pub mod raw {
    pub use gpui_kit::*;
}

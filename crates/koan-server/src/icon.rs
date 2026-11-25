//! The kōan icon set, as the web UI and the share page draw it: the web kit's (`koan_kit_web::icon`), which
//! `just icons` compiles from the same SVGs the apps draw. Templates draw a glyph with
//! `{{ crate::icon::HEART.svg(15.0).class("…") }}`.

pub use koan_kit_web::icon::*;

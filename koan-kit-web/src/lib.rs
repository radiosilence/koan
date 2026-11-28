//! The kōan web kit as askama components, wired for Datastar.
//!
//! The kit's stylesheet (`theme.css`, which this crate carries) draws plain elements in the kōan look and gives a
//! handful of classes for the rest. This crate writes the markup around those classes once, so every app draws a
//! field, a sheet or a busy button the same way, with the same accessibility and the same Datastar wiring, rather than
//! each app writing its own and each drifting. It is to the web what koan-kit is to GPUI.
//!
//! # Components
//!
//! A component is a value: built with its props, then rendered wherever askama renders a value. In a view:
//!
//! ```
//! use koan_kit_web as kit;
//!
//! let save = kit::Button::primary("save").post("/settings");
//! let state = kit::Status::ok("connected");
//! assert_eq!(state.to_string(), r#"<span class="status ok">connected</span>"#);
//! ```
//!
//! or inline, in a template whose struct's module has `use koan_kit_web as kit;`:
//!
//! ```html
//! {{ kit::Field::email("email", "email").required().hint("For sign-in only.").error(error) }}
//! {{ kit::Button::quiet("remove").bad().post(url).confirm("remove this album?", "remove") }}
//! ```
//!
//! Every prop takes what a view holds, by value or by reference: askama hands a template's variables over by
//! reference and its literals by value, and [`Text`], [`Amount`], [`Flag`] and [`Content`] convert from either, so a
//! template never writes `*count` or `&name`. Text is escaped wherever it lands. Markup is a separate type that no
//! string converts into, so a person's text cannot be passed where markup goes.
//!
//! The common props, on every component: [`id`](Button::id), [`class`](Button::class) for the app's own layout
//! utilities, [`data`](Button::data), [`attr`](Button::attr) and [`datastar`](Button::datastar). And on every
//! component, [`html`](Button::html) and [`patch`](Button::patch), so a handler answers with one:
//! `events([kit::Flash::ok("Saved.").patch().into()])`.
//!
//! # Children
//!
//! A component with a slot (a [`Sheet`]'s body, a [`Disclosure`]'s, a [`Notice`]'s words, a [`Field`]'s control) takes
//! its content as a prop, or from the template that places it, through the [`wrap`] filter:
//!
//! ```html
//! {% filter kit::wrap(kit::Sheet::new("add", "add a download").close("close")) %}
//!   <form …>…</form>
//! {% endfilter %}
//! ```
//!
//! # Datastar
//!
//! The components write Datastar's attributes in the syntax of the release [`askama_datastar`] serves, which is why
//! this crate re-exports it: an app takes both together. Every expression is the kit's own constant. A URL or a
//! question travels in a `data-*` attribute, escaped, and the expression reads it with `el.dataset`, so nothing a
//! person typed reaches script (§8). A control that sends a request is busy, disabled and spinning, for exactly as long
//! as its own request runs. A question before a change goes through the page's one [`Confirm`].
//!
//! # The stylesheet
//!
//! The crate carries the kit's CSS: `theme.css`, `tokens.css`, `kit.css`, `kit-check.mjs` and the font. An app's
//! `just css` finds the crate's source with `cargo metadata` and imports `theme.css` from there, so the version in
//! `Cargo.lock` names both the markup and the rules that draw it. The components use the kit's classes alone, never a
//! utility an app's Tailwind would have to find, and a test fails on a class the kit has no rule for.

mod action;
mod arg;
mod attrs;
mod button;
mod choice;
mod content;
mod cover;
mod field;
pub mod font;
mod form;
mod head;
pub mod icon;
mod link;
mod message;
mod navigation;
mod qr;
mod repeat;
mod selection;
mod sheet;
mod slot;
mod spinner;
mod status;
#[cfg(any(test, feature = "storybook"))]
pub mod storybook;
mod tabs;
mod time;

pub use askama_datastar;

pub use arg::{Amount, AsText, Content, Flag, Markup, Text};
pub use button::{Button, Weight};
pub use choice::{Check, Toggle};
pub use content::{CopyField, Disclosure, List, Pairs, ScrollsIntoView, scrolls_into_view};
pub use cover::Cover;
pub use field::{Choice, Combo, Control, Field, Input, InputKind, Select, Textarea};
pub use form::{Form, FormAttrs};
pub use head::{Footer, Head, ImportMap, Release, SkipLink, TopBar, Wordmark};
pub use link::Link;
pub use message::{Empty, Flash, Notice};
pub use navigation::Navigation;
pub use qr::Qr;
pub use repeat::RepeatField;
pub use selection::{Selection, SelectionBar, SelectionCount};
pub use sheet::{CONFIRMED, Confirm, Sheet};
pub use slot::{Wrap, wrap};
pub use spinner::Spinner;
pub use status::{Meter, Status, Tone};
pub use tabs::{Tab, Tabs};
pub use time::{Epoch, Time};

#[cfg(test)]
mod tests {
    #[test]
    fn the_crate_and_the_npm_package_are_one_version() {
        let package = include_str!("../package.json");
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            package.contains(&format!("\"version\": \"{version}\"")),
            "package.json and Cargo.toml must carry one version, {version}: they publish the one kit"
        );
    }
}

//! Navigation in place for a whole page: one listener that loads links in place, and Back and Forward that do the
//! same rather than reloading.
//!
//! [`Link::in_place`](crate::Link::in_place) writes its own listener, which suits a handful of links. A page holding
//! thousands (an album grid, a track list) would carry one per link, so [`Navigation`] listens once, on the window,
//! for clicks on links inside any element marked [`Navigation::SCOPE`]. A page that holds something a reload would
//! stop, such as a player, needs Back and Forward to load in place too, which `Navigation` does for every entry.

use askama::Template;

use crate::arg::Text;
use crate::attrs::{Attrs, component};
use crate::field::signal_name;

/// A page's navigation in place: `kit::Navigation::new().follows("url")`, placed once in the shell.
///
/// A plain click on a link inside an element carrying [`SCOPE`](Self::SCOPE) reads its address with `@get`, the
/// handler patching what changes, as [`Link::in_place`](crate::Link::in_place) does: a modified or middle click, a
/// link with a `target` or `download`, one marked `data-whole`, one to another origin and one to a fragment of this
/// page are left to the browser. The pressed link is drawn busy until its answer has arrived, and a press on another
/// link meanwhile abandons the first. Back and Forward read the entry's address in place rather than reloading, and
/// `Link::in_place`'s own reload on Back stands down while a `Navigation` is on the page.
///
/// Without [`follows`](Self::follows), each load pushes the link's address onto the history. With it, the address is
/// the signal's, which the handler's answer sets to the address it drew (`askama_datastar::address`), and whatever
/// moves the signal moves the address bar: it does what `askama_datastar::history` does, which a page using this
/// leaves out, since that one reloads on Back.
#[must_use = "navigation does nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "navigation.html")]
pub struct Navigation<'a> {
    follows: Option<&'static str>,
    attrs: Attrs<'a>,
}

impl Navigation<'_> {
    /// The attribute that marks an element whose links load in place: `<main data-kit-in-place>`.
    pub const SCOPE: &'static str = "data-kit-in-place";

    /// The id of its element, which `Link::in_place` looks for.
    pub const ID: &'static str = "kit-navigation";

    /// Links inside an element marked [`SCOPE`](Self::SCOPE) load in place, and Back and Forward load in place.
    pub fn new() -> Self {
        let mut attrs = Attrs::default();
        attrs.set_id(Text::from(Self::ID));
        Self {
            follows: None,
            attrs,
        }
    }

    /// Keeps the address bar on the address in `signal`, which each answer sets to the page it drew.
    ///
    /// # Panics
    ///
    /// When `signal` is not letters, digits, `_` and `.`: it is written into the expressions.
    pub fn follows(mut self, signal: &'static str) -> Self {
        self.follows = Some(signal_name(signal));
        self
    }
}

impl Default for Navigation<'_> {
    fn default() -> Self {
        Self::new()
    }
}

component!(Navigation);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_listener_loads_the_links_of_a_marked_region() {
        let html = Navigation::new().to_string();
        assert!(html.starts_with("<div hidden"), "{html}");
        assert!(html.contains("id=\"kit-navigation\""), "{html}");
        assert!(html.contains("data-on:click__window="), "{html}");
        assert!(
            html.contains(
                "[data-kit-in-place] a[href]:not([target]):not([download]):not([data-whole])"
            ),
            "{html}"
        );
        assert!(
            html.contains("evt.metaKey || evt.ctrlKey || evt.shiftKey || evt.altKey || evt.button"),
            "a modified click is the browser's: {html}"
        );
        assert!(
            html.contains("history.pushState({kit: 1}, '', a.href)"),
            "without a signal, a load pushes the link's address: {html}"
        );
        assert!(
            html.contains("data-on:popstate__window="),
            "Back loads in place: {html}"
        );
        assert!(!html.contains("reload"), "{html}");
    }

    #[test]
    fn following_a_signal_leaves_the_address_to_it() {
        let html = Navigation::new().follows("url").to_string();
        assert!(
            html.contains("$url &amp;&amp; $url != location.pathname + location.search"),
            "{html}"
        );
        assert!(
            !html.contains("a.href)"),
            "the click leaves the address to the answer: {html}"
        );
    }

    #[test]
    #[should_panic(expected = "is not a signal name")]
    fn a_signal_name_is_the_programs() {
        let _ = Navigation::new().follows("url) || alert(1");
    }
}

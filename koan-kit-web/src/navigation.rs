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

/// A page's navigation in place, and the one owner of its history: `kit::Navigation::new()`, placed once in the shell.
///
/// A plain click on a link inside an element carrying [`SCOPE`](Self::SCOPE), or on a
/// [`Link::in_place`](crate::Link::in_place) or in-place [`Button`](crate::Button) anywhere on the page, reads its
/// address with `@get`, the handler patching what changes: a modified or middle click, a link with a `target` or
/// `download`, one marked `data-whole`, one to another origin and one to a fragment of this page are left to the
/// browser. The pressed link is drawn busy until its answer has arrived, and a press on another link meanwhile
/// abandons the first; a link's `busy_key` does not hold it back here, since a newer load replaces the older.
///
/// The address is the signal's ([`follows`](Self::follows), `url` unless named), which each answer sets to the address
/// it drew (`askama_datastar::address`). Once a load has settled, that address is what the page shows, and it is pushed
/// when the bar shows another, so a load that fails moves nothing. Back and Forward read the entry's address in place
/// rather than reloading, unless the page already shows it; a load that fails there replaces the entry with the address
/// the page does show. It does what `askama_datastar::history` does, which a page using it leaves out, since that one
/// reloads on Back; every signal it follows must be declared on the page.
#[must_use = "navigation does nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "navigation.html")]
pub struct Navigation<'a> {
    follows: &'static str,
    attrs: Attrs<'a>,
}

impl Navigation<'_> {
    /// The attribute that marks an element whose links load in place: `<main data-kit-in-place>`.
    pub const SCOPE: &'static str = "data-kit-in-place";

    /// The id of its element, which in-place links look for.
    pub const ID: &'static str = "kit-navigation";

    /// Links load in place, and Back and Forward load in place, the address following the signal `url`.
    pub fn new() -> Self {
        let mut attrs = Attrs::default();
        attrs.set_id(Text::from(Self::ID));
        Self {
            follows: "url",
            attrs,
        }
    }

    /// Follows the signal `signal` in place of `url`.
    ///
    /// # Panics
    ///
    /// When `signal` is not letters, digits, `_` and `.`: it is written into the expressions.
    pub fn follows(mut self, signal: &'static str) -> Self {
        self.follows = signal_name(signal);
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
    use crate::link::Link;

    #[test]
    fn one_listener_loads_the_links_of_a_marked_region_and_in_place_links() {
        let html = Navigation::new().to_string();
        assert!(html.starts_with("<div hidden"), "{html}");
        assert!(html.contains("id=\"kit-navigation\""), "{html}");
        assert!(
            html.contains(":is([data-kit-in-place] a[href], a[href][data-in-place]):not([target]):not([download]):not([data-whole])"),
            "{html}"
        );
        assert!(
            html.contains("evt.metaKey || evt.ctrlKey || evt.shiftKey || evt.altKey || evt.button"),
            "a modified click is the browser's: {html}"
        );
        assert!(html.contains("data-on:popstate__window="), "{html}");
        assert!(!html.contains("reload"), "{html}");
    }

    #[test]
    fn nothing_is_pushed_until_an_answer_names_the_page() {
        let html = Navigation::new().to_string();
        let (_, click) = html
            .split_once("data-on:click__window=")
            .expect("a click listener");
        let click = click.split("data-on:popstate").next().unwrap_or_default();
        assert!(
            !click.contains("pushState"),
            "a failed load must leave the address: {click}"
        );
        assert!(
            !click.contains("_kitshown"),
            "only an answer says what the page shows: {click}"
        );
        assert!(
            html.contains("$_kitnavbusy || (") && html.contains("el._kitshown = $url"),
            "the settled answer's address is what the page shows: {html}"
        );
        assert!(
            html.contains("el._kitpopped ? history.replaceState({kit: 1}, '', $url)"),
            "a Back that failed corrects its entry rather than adding one: {html}"
        );
        let other = Navigation::new().follows("page.url").to_string();
        assert!(other.contains("$page.url &amp;&amp;"), "{other}");
    }

    #[test]
    fn an_in_place_link_leaves_its_click_to_the_navigation() {
        let link = Link::new("b", "/b").in_place().to_string();
        assert!(link.contains(" data-in-place "), "{link}");
        assert!(
            link.contains(
                "data-on:click=\"document.getElementById('kit-navigation') || evt.metaKey"
            ),
            "{link}"
        );
    }

    #[test]
    #[should_panic(expected = "is not a signal name")]
    fn a_signal_name_is_the_programs() {
        let _ = Navigation::new().follows("url) || alert(1");
    }
}

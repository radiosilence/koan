//! A link in the page's own look: one that goes somewhere, or loads its page in place.

use std::fmt;

use askama::Template;

use crate::action::Busy;
use crate::arg::{Content, Flag, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::button::in_place_attributes;
use crate::slot::Wrap;

/// A link, drawn as the page draws its links: `kit::Link::new("Untrue", url).in_place()`.
///
/// A link drawn as a button is a [`Button`](crate::Button) with [`href`](crate::Button::href), which loads in place
/// the same way.
#[must_use = "a link draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "link.html")]
pub struct Link<'a> {
    label: Content<'a>,
    href: Text<'a>,
    in_place: bool,
    busy_key: Text<'a>,
    current: bool,
    card: bool,
    attrs: Attrs<'a>,
}

impl<'a> Link<'a> {
    /// A link to `href`, saying `label`.
    pub fn new(label: impl Into<Text<'a>>, href: impl Into<Text<'a>>) -> Self {
        Self {
            label: Content::Text(label.into()),
            href: href.into(),
            in_place: false,
            busy_key: Text::default(),
            current: false,
            card: false,
            attrs: Attrs::default(),
        }
    }

    /// A link to `href` holding a block, such as a cover over a title and a count: a card. Its content is the markup of
    /// a `{% filter kit::wrap(kit::Link::to(url).in_place()) %}` block, and it draws in the text's colour with no
    /// underline. On a page with a [`Navigation`](crate::Navigation), a plain `<a>` inside its scope loads in place
    /// without `in_place`, which suits a grid of thousands.
    pub fn to(href: impl Into<Text<'a>>) -> Self {
        let mut link = Self::new("", href);
        link.card = true;
        link
    }

    /// A plain click reads the page in place with `@get`, its handler patching what changes, while a modified click, a
    /// middle click or a page without script follows the real `href`. The spinner follows it while it loads, and a
    /// second press meanwhile does nothing: crumbs, a tree to browse, a list of conversations. The address becomes the
    /// `href` in a history entry of its own, and Back to an entry made this way reloads it, so a reload, Back and a
    /// copied address all name the page shown.
    pub fn in_place(mut self) -> Self {
        self.in_place = true;
        self
    }

    /// Joins the group `key` names, as [`Button::busy_key`](crate::Button::busy_key): while any of its requests runs, a
    /// press on any link of the group does nothing, so a row of crumbs loads one page at a time.
    pub fn busy_key(mut self, key: impl Into<Text<'a>>) -> Self {
        self.busy_key = key.into();
        self
    }

    /// The page being shown, marked `aria-current="page"`: the tab or crumb the reader is on.
    pub fn current(mut self, on: impl Into<Flag>) -> Self {
        self.current = on.into().get();
        self
    }

    /// Its classes: a card's, then the app's.
    fn classes(&self) -> Option<String> {
        let classes: Vec<&str> = self
            .card
            .then_some("card")
            .into_iter()
            .chain(self.attrs.class_list())
            .collect();
        (!classes.is_empty()).then(|| classes.join(" "))
    }

    /// The attributes that load it in place, when it does.
    fn loads(&self) -> InPlace<'_, 'a> {
        InPlace(self)
    }
}

/// A link's in-place attributes, written into its tag.
struct InPlace<'l, 'a>(&'l Link<'a>);

impl fmt::Display for InPlace<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let link = self.0;
        if link.in_place {
            let own = Busy::own(&[
                "link",
                link.href.as_str(),
                link.busy_key.as_str(),
                link.attrs.id(),
            ]);
            in_place_attributes(f, own, &link.busy_key)?;
        }
        Ok(())
    }
}

impl askama::filters::HtmlSafe for InPlace<'_, '_> {}

impl Wrap for Link<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.label = content.into();
        self
    }
}

component!(Link);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_holds_a_block_and_loads_in_place() {
        let html = Link::to("/albums/7")
            .in_place()
            .wrap(Markup::kit(
                "<img alt=\"\" src=\"/c/7\"><strong>Untrue</strong>".into(),
            ))
            .to_string();
        assert!(
            html.starts_with("<a href=\"/albums/7\" class=\"card\""),
            "{html}"
        );
        assert!(html.contains("@get(el.getAttribute('href')"), "{html}");
        assert!(
            html.ends_with("><img alt=\"\" src=\"/c/7\"><strong>Untrue</strong></a>"),
            "{html}"
        );
    }

    #[test]
    fn a_link_in_place_leaves_a_modified_click_to_the_browser() {
        let html = Link::new("Untrue", "/browse?path=a&b")
            .in_place()
            .busy_key("browse")
            .current(true)
            .to_string();
        assert!(
            html.starts_with("<a href=\"/browse?path=a&#38;b\""),
            "{html}"
        );
        assert!(html.contains("aria-current=\"page\""), "{html}");
        assert!(
            html.contains("evt.metaKey || evt.ctrlKey || evt.shiftKey || evt.altKey || evt.button"),
            "{html}"
        );
        assert!(html.contains("@get(el.getAttribute('href')"), "{html}");
        assert!(
            html.contains("history.pushState({kit: 1}, '', el.href)"),
            "the address follows the page: {html}"
        );
        assert!(html.ends_with(">Untrue</a>"), "{html}");
    }
}

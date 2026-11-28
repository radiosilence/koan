//! Tabs: a row of addresses, the one shown marked, each loading in place, with actions of its own.

use askama::Template;

use crate::arg::{Flag, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::button::Button;
use crate::link::Link;
use crate::slot::Wrap;
use crate::status::Tone;

/// A row of tabs, named for a screen reader: `kit::Tabs::new("searches").tab(kit::Tab::new("Burial", url))`.
///
/// Each tab is an address, so it is a link: the one shown carries `aria-current="page"`, a modified click opens a tab
/// of the browser's, and an [`in_place`](Tab::in_place) tab loads its page in place. The row wraps when it runs out of
/// room.
#[must_use = "tabs draw nothing until they are rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "tabs.html")]
pub struct Tabs<'a> {
    label: Text<'a>,
    each: Vec<Tab<'a>>,
    attrs: Attrs<'a>,
}

impl<'a> Tabs<'a> {
    /// No tabs yet, the row named `label`.
    pub fn new(label: impl Into<Text<'a>>) -> Self {
        Self {
            label: label.into(),
            each: Vec::new(),
            attrs: Attrs::default(),
        }
    }

    /// A tab, after those already given.
    pub fn tab(mut self, tab: Tab<'a>) -> Self {
        self.each.push(tab);
        self
    }

    /// Tabs, after those already given: `searches.iter().map(…)`.
    pub fn tabs(mut self, tabs: impl IntoIterator<Item = Tab<'a>>) -> Self {
        self.each.extend(tabs);
        self
    }
}

impl<'a> FromIterator<Tab<'a>> for Tabs<'a> {
    fn from_iter<I: IntoIterator<Item = Tab<'a>>>(tabs: I) -> Self {
        Tabs::new("").tabs(tabs)
    }
}

impl Wrap for Tabs<'_> {
    /// Tabs a template builds with a loop, after those given as props.
    fn wrap(mut self, content: Markup) -> Self {
        self.each.push(Tab::markup(content));
        self
    }
}

component!(Tabs);

/// One tab of [`Tabs`]: an address and what it says, with actions after it, such as pin and close.
#[must_use = "a tab draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "tab.html")]
pub struct Tab<'a> {
    link: Option<Link<'a>>,
    current: bool,
    busy: bool,
    tone: Option<Tone>,
    count: Text<'a>,
    actions: Vec<Button<'a>>,
    /// Tabs a template wrote, standing in for this one.
    markup: Markup,
    attrs: Attrs<'a>,
}

impl<'a> Tab<'a> {
    /// A tab saying `label`, for the page at `href`.
    pub fn new(label: impl Into<Text<'a>>, href: impl Into<Text<'a>>) -> Self {
        Self {
            link: Some(Link::new(label, href)),
            current: false,
            busy: false,
            tone: None,
            count: Text::default(),
            actions: Vec::new(),
            markup: Markup::default(),
            attrs: Attrs::default(),
        }
    }

    fn markup(markup: Markup) -> Self {
        Self {
            link: None,
            current: false,
            busy: false,
            tone: None,
            count: Text::default(),
            actions: Vec::new(),
            markup,
            attrs: Attrs::default(),
        }
    }

    /// The page being shown.
    pub fn current(mut self, on: impl Into<Flag>) -> Self {
        let on = on.into().get();
        self.current = on;
        self.link = self.link.map(|link| link.current(on));
        self
    }

    /// Loads its page in place, as [`Link::in_place`] does.
    pub fn in_place(mut self) -> Self {
        self.link = self.link.map(Link::in_place);
        self
    }

    /// Joins the busy group `key` names, as [`Link::busy_key`] does: a row of tabs loads one page at a time.
    pub fn busy_key(mut self, key: impl Into<Text<'a>>) -> Self {
        self.link = self.link.map(|link| link.busy_key(key));
        self
    }

    /// Its page is still working, such as a search still running: the ensō turns after its words, and the tab is
    /// busy to a screen reader.
    pub fn busy(mut self, on: impl Into<Flag>) -> Self {
        self.busy = on.into().get();
        self
    }

    /// Its words in a tone, as a [`Status`](crate::Status) of that tone draws its own: `Bad` for a search that failed,
    /// `Warn` for one that needs attention, `Ok` for one that is done, `Wait` for one queued. `None`, or a tone never
    /// given, leaves its words as a tab's are, so a view passes the `Option` it worked out as it is.
    pub fn tone(mut self, tone: impl Into<Option<Tone>>) -> Self {
        self.tone = tone.into();
        self
    }

    fn tone_class(&self) -> String {
        self.tone
            .map_or_default(|tone| format!(" {}", tone.class()))
    }

    /// A figure after its words: how many results it holds.
    pub fn count(mut self, count: impl Into<Text<'a>>) -> Self {
        self.count = count.into();
        self
    }

    /// An action after it, such as pin or close: a quiet button, compact and usually a glyph alone, posting where the
    /// tab's handler is.
    pub fn action(mut self, button: Button<'a>) -> Self {
        self.actions.push(button);
        self
    }
}

component!(Tab);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon;

    #[test]
    fn the_tab_shown_is_marked_and_each_is_a_link() {
        let html = Tabs::new("searches")
            .tab(Tab::new("Burial", "/search/1").current(true).in_place())
            .tab(
                Tab::new("Phace", "/search/2").count(12).in_place().action(
                    Button::quiet("close")
                        .icon_only(icon::CLOSE)
                        .compact()
                        .post("/search/2/close"),
                ),
            )
            .to_string();
        assert!(
            html.starts_with("<nav class=\"tabs\" aria-label=\"searches\">"),
            "{html}"
        );
        assert!(html.contains("<span class=\"tab current\">"), "{html}");
        assert_eq!(html.matches("aria-current=\"page\"").count(), 1, "{html}");
        assert!(html.contains("@get(el.getAttribute('href')"), "{html}");
        assert!(html.contains("<small>12</small>"), "{html}");
        assert!(html.contains("data-url=\"/search/2/close\""), "{html}");
    }

    #[test]
    fn a_tab_can_be_set_in_italic() {
        let stale = Tab::new("Burial", "/search/1").italic().to_string();
        assert!(stale.starts_with("<span class=\"tab italic\">"), "{stale}");
        let both = Tab::new("Phace", "/search/2")
            .tone(Tone::Bad)
            .italic()
            .to_string();
        assert!(
            both.starts_with("<span class=\"tab bad italic\">"),
            "a slant goes with a tone: {both}"
        );
        let css = include_str!("../kit.css");
        assert!(
            css.contains(".italic {\n    @apply italic;"),
            "the kit draws its own class"
        );
    }

    #[test]
    fn a_tab_says_it_is_working_and_takes_a_tone() {
        let html = Tab::new("Burial", "/search/1").busy(true).to_string();
        assert!(
            html.starts_with("<span class=\"tab\" aria-busy=\"true\">"),
            "{html}"
        );
        assert!(html.contains("class=\"spinner\""), "{html}");
        let failed = Tab::new("Phace", "/search/2").tone(Tone::Bad).to_string();
        assert!(failed.starts_with("<span class=\"tab bad\">"), "{failed}");
        assert!(!failed.contains("spinner"), "{failed}");
        assert!(
            Tab::new("Phace", "/")
                .tone(Tone::Wait)
                .to_string()
                .starts_with("<span class=\"tab wait\">"),
            "Wait is a tone like the others, not a way to say none"
        );
        assert!(
            Tab::new("Phace", "/")
                .tone(None)
                .to_string()
                .starts_with("<span class=\"tab\">"),
            "None is no tone"
        );
        let css = include_str!("../kit.css");
        assert!(css.contains(".tab.wait > a {"), "every tone has a rule");
    }
}

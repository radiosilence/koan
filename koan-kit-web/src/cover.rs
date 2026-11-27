//! A region covered while a busy group's requests run.

use askama::Template;

use crate::action::Busy;
use crate::arg::{Flag, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::slot::Wrap;

/// What it holds, covered while any request of the group `key` names runs: faded, taking no input, and the ensō turning
/// over it. `{% filter kit::wrap(kit::Cover::with("picker")) %}…{% endfilter %}`.
///
/// The group is the one controls join with [`Button::busy_key`](crate::Button::busy_key) and
/// [`Link::busy_key`](crate::Link::busy_key): a folder picker whose folders load the next listing covers the listing
/// being left, so it is not read or pressed while it is on its way out. The region is `aria-busy` meanwhile.
#[must_use = "a cover draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "cover.html")]
pub struct Cover<'a> {
    content: Markup,
    key: Text<'a>,
    busy: bool,
    attrs: Attrs<'a>,
}

impl<'a> Cover<'a> {
    /// Covered while the group `key` names is busy.
    pub fn with(key: impl Into<Text<'a>>) -> Self {
        Self {
            content: Markup::default(),
            key: key.into(),
            busy: false,
            attrs: Attrs::default(),
        }
    }

    /// What it holds, when it is not the markup of a `wrap` block.
    pub fn content(mut self, content: impl Into<Markup>) -> Self {
        self.content = content.into();
        self
    }

    /// Covered from the start, for work the server knows is still under way.
    pub fn busy(mut self, on: impl Into<Flag>) -> Self {
        self.busy = on.into().get();
        self
    }

    /// The group's signal.
    fn signal(&self) -> Busy {
        Busy::keyed(self.key.as_str())
    }
}

impl Wrap for Cover<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.content = content;
        self
    }
}

component!(Cover);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::Button;

    #[test]
    fn a_cover_follows_its_groups_signal() {
        let group = Busy::keyed("picker");
        let html = Cover::with("picker")
            .content(
                Button::bordered("music")
                    .get("/pick?music")
                    .busy_key("picker"),
            )
            .to_string();
        assert!(html.starts_with("<div class=\"cover\""), "{html}");
        assert!(
            html.contains(&format!(
                "data-attr:aria-busy=\"${group} ? 'true' : 'false'\""
            )),
            "{html}"
        );
        assert!(
            html.contains(&format!("data-attr:inert=\"${group}\"")),
            "{html}"
        );
        assert!(
            html.contains(&format!("data-indicator:{group}")),
            "the button joins the group: {html}"
        );
    }
}

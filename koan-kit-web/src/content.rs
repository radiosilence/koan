//! Content in the kit's shapes: labelled [`Pairs`], a [`Disclosure`] that opens, a [`CopyField`] to take a value from.

use askama::Template;

use crate::arg::{Content, Flag, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::slot::Wrap;

/// Labels and their values, the labels muted and aligned: `kit::Pairs::new().pair("size", "4.2 GB")`.
///
/// Collects from an iterator of pairs, so a handler builds one from its rows: `rows.iter().map(…).collect()`.
#[must_use = "pairs draw nothing until they are rendered"]
#[derive(Clone, Debug, Default, Template)]
#[template(path = "pairs.html")]
pub struct Pairs<'a> {
    pairs: Vec<(Text<'a>, Content<'a>)>,
    attrs: Attrs<'a>,
}

impl<'a> Pairs<'a> {
    /// None yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// `label` and its value, after those already given. The value is words, or markup such as a link.
    pub fn pair(mut self, label: impl Into<Text<'a>>, value: impl Into<Content<'a>>) -> Self {
        self.pairs.push((label.into(), value.into()));
        self
    }
}

impl<'a, L: Into<Text<'a>>, V: Into<Content<'a>>> FromIterator<(L, V)> for Pairs<'a> {
    fn from_iter<I: IntoIterator<Item = (L, V)>>(pairs: I) -> Self {
        pairs
            .into_iter()
            .fold(Self::new(), |all, (label, value)| all.pair(label, value))
    }
}

component!(Pairs);

/// A summary that opens onto more: `kit::Disclosure::new("files")`.
///
/// Its body is [`body`](Self::body), or the markup of a `{% filter kit::wrap(disclosure) %}` block. One that
/// [`loads`](Self::loads) fetches its body when first opened: the handler answers with the same disclosure, under the
/// same [`id`](Self::id), open and holding what it read, so one component draws both.
#[must_use = "a disclosure draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "disclosure.html")]
pub struct Disclosure<'a> {
    summary: Content<'a>,
    body: Content<'a>,
    open: bool,
    loads: Text<'a>,
    reloads: bool,
    attrs: Attrs<'a>,
}

impl<'a> Disclosure<'a> {
    /// Closed, summarised by `summary`.
    pub fn new(summary: impl Into<Content<'a>>) -> Self {
        Self {
            summary: summary.into(),
            body: Content::default(),
            open: false,
            loads: Text::default(),
            reloads: false,
            attrs: Attrs::default(),
        }
    }

    /// What it opens onto.
    pub fn body(mut self, body: impl Into<Content<'a>>) -> Self {
        self.body = body.into();
        self
    }

    /// Open.
    pub fn open(mut self, on: impl Into<Flag>) -> Self {
        self.open = on.into().get();
        self
    }

    /// Fetches its body from `url` the first time it opens, showing `placeholder` until then. It needs an
    /// [`id`](Self::id) for the answer to find it.
    pub fn loads(mut self, url: impl Into<Text<'a>>, placeholder: impl Into<Content<'a>>) -> Self {
        self.loads = url.into();
        self.body = placeholder.into();
        self
    }
}

impl Disclosure<'_> {
    /// Fetches its body again every time it opens, not only the first: for what changes while the page is open.
    pub fn reloads(mut self) -> Self {
        self.reloads = true;
        self
    }
}

impl Wrap for Disclosure<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.body = content.into();
        self
    }
}

component!(Disclosure);

/// A value to read or copy, beside the button that copies it: a token, a URL, a command.
#[must_use = "a copy field draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "copy.html")]
pub struct CopyField<'a> {
    label: Text<'a>,
    value: Text<'a>,
    button: Text<'a>,
    copied: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> CopyField<'a> {
    /// `value`, named `label` for a screen reader, with a button saying `button`.
    pub fn new(
        label: impl Into<Text<'a>>,
        value: impl Into<Text<'a>>,
        button: impl Into<Text<'a>>,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            button: button.into(),
            copied: Text::default(),
            attrs: Attrs::default(),
        }
    }

    /// What the button says for two seconds once the value is on the clipboard.
    pub fn copied(mut self, label: impl Into<Text<'a>>) -> Self {
        self.copied = label.into();
        self
    }
}

component!(CopyField);

#[cfg(test)]
mod tests {
    use askama_datastar::Html;

    use super::*;

    #[test]
    fn pairs_take_words_and_markup() {
        let pairs: Pairs<'_> = [
            ("size", Content::from("4 GB")),
            ("source", Html::trusted("<a href=\"/x\">x</a>").into()),
        ]
        .into_iter()
        .collect();
        let html = pairs.to_string();
        assert!(html.contains("<dt>size</dt><dd>4 GB</dd>"), "{html}");
        assert!(html.contains("<dd><a href=\"/x\">x</a></dd>"), "{html}");
    }

    #[test]
    fn a_disclosure_that_loads_fetches_once_with_a_constant_expression() {
        let html = Disclosure::new("files")
            .loads("/jobs/1/files", "Reading…")
            .id("files-1")
            .to_string();
        assert!(html.contains("data-url=\"/jobs/1/files\""), "{html}");
        assert!(
            html.contains("data-on:toggle__once=\"el.open &amp;&amp; @get(el.dataset.url)\""),
            "{html}"
        );
    }

    #[test]
    fn a_copy_field_copies_through_its_own_data() {
        let html = CopyField::new("api token", "s3cr'et", "copy")
            .copied("copied")
            .to_string();
        assert!(html.contains("value=\"s3cr&#39;et\""), "{html}");
        assert!(html.contains("data-copied=\"copied\""), "{html}");
        assert!(
            !html.contains("s3cr'et"),
            "the value reaches no expression: {html}"
        );
    }
}

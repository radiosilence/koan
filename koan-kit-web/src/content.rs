//! Content in the kit's shapes: labelled [`Pairs`], a [`Disclosure`] that opens, a [`CopyField`] to take a value from.

use askama::Template;

use crate::arg::{Content, Flag, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::icon;
use crate::slot::Wrap;

/// Labels and their values, the labels muted and aligned: `kit::Pairs::new().pair("size", "4.2 GB")`.
///
/// Collects from an iterator of pairs, so a handler builds one from its rows: `rows.iter().map(…).collect()`. A row
/// that is there only sometimes is [`optional`](Self::optional); rows a template builds with loops are the markup of a
/// `{% filter kit::wrap(pairs) %}` block, `<dt>` and `<dd>` elements, after those given as props.
#[must_use = "pairs draw nothing until they are rendered"]
#[derive(Clone, Debug, Default, Template)]
#[template(path = "pairs.html")]
pub struct Pairs<'a> {
    pairs: Vec<(Text<'a>, Content<'a>)>,
    rows: Markup,
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

    /// `label` and its value, only when there is a value: an `Option` or an empty list passes as it is.
    pub fn optional(self, label: impl Into<Text<'a>>, value: impl Into<Content<'a>>) -> Self {
        let value = value.into();
        if value.is_empty() {
            self
        } else {
            self.pair(label, value)
        }
    }
}

impl Wrap for Pairs<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.rows = content;
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
/// Its body is [`body`](Self::body), or the markup of a `{% filter kit::wrap(disclosure) %}` block. A block that opens
/// with a `<summary>` element gives the summary too, as markup with the template's own layout, in place of the words
/// given to [`new`](Self::new), which may then be empty:
///
/// ```html
/// {% filter kit::wrap(kit::Disclosure::new("")) %}
///   <summary class="flex gap-2"><span>{{ title }}</span><span class="text-muted">{{ length }}</span></summary>
///   <table class="dense">…</table>
/// {% endfilter %}
/// ```
///
/// One that
/// [`loads`](Self::loads) fetches its body when it opens: the handler answers with the same disclosure, under the
/// same [`id`](Self::id), open and holding what it read and without `loads`, so one component draws both and it stops
/// fetching. For a body that changes while the page is open, answer with [`Disclosure::fill`] instead, which replaces
/// the body alone and keeps the element, so it fetches again on every opening. Never answer with the disclosure open
/// and still loading: putting an open `<details>` in place fires its toggle, and it would fetch without end.
#[must_use = "a disclosure draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "disclosure.html")]
pub struct Disclosure<'a> {
    summary: Content<'a>,
    /// A `<summary>` element of the template's own, which leads a wrapped block.
    own_summary: Markup,
    body: Content<'a>,
    open: bool,
    loads: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Disclosure<'a> {
    /// Closed, summarised by `summary`.
    pub fn new(summary: impl Into<Content<'a>>) -> Self {
        Self {
            summary: summary.into(),
            own_summary: Markup::default(),
            body: Content::default(),
            open: false,
            loads: Text::default(),
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
    /// A loading handler's answer that replaces the body of the disclosure `id` alone, keeping the element and its
    /// `loads`, so it fetches again when next opened: `events([kit::Disclosure::fill("files-1", list).into()])`.
    pub fn fill<'c>(id: &str, body: impl Into<Content<'c>>) -> askama_datastar::PatchElements {
        // Replaced, not morphed: Datastar morphs nothing inside an element that keeps `data-ignore-morph`, as a
        // disclosure in a streamed list does, and a replacement it makes.
        let body: Content<'c> = body.into();
        askama_datastar::Html::trusted(format!("<div>{body}</div>"))
            .patch()
            .selector(format!("#{id} > div"))
            .mode(askama_datastar::ElementPatchMode::Replace)
    }
}

impl Disclosure<'_> {
    /// Whether its body is a placeholder for what it will load.
    fn loading(&self) -> bool {
        !self.loads.is_empty()
    }
}

impl Wrap for Disclosure<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        let markup = content.as_str().trim_start();
        let end = "</summary>";
        if markup.starts_with("<summary")
            && let Some(at) = markup.find(end)
        {
            let (summary, body) = markup.split_at(at + end.len());
            self.own_summary = Markup::kit(summary.to_owned());
            self.body = Markup::kit(body.trim_start().to_owned()).into();
        } else {
            self.body = content.into();
        }
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
    name: Text<'a>,
    icon: bool,
    compact: bool,
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
            name: Text::default(),
            icon: false,
            compact: false,
            attrs: Attrs::default(),
        }
    }

    /// What the button says for two seconds once the value is on the clipboard; in the glyph form, what it is named
    /// meanwhile, as a tick stands in for the glyph.
    pub fn copied(mut self, label: impl Into<Text<'a>>) -> Self {
        self.copied = label.into();
        self
    }

    /// The field's name, which it submits with a form it sits in. Its id is the component's [`id`](Self::id) and
    /// `-value`, so a browser's checks find a name or an id on every field.
    pub fn name(mut self, name: impl Into<Text<'a>>) -> Self {
        self.name = name.into();
        self
    }

    /// The button as the copy glyph alone, named by its label, for a row with no room for the word.
    pub fn icon_only(mut self) -> Self {
        self.icon = true;
        self
    }

    /// The size of a compact field.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// The input's id, when the component has one.
    fn input_id(&self) -> Option<String> {
        (!self.attrs.id().is_empty()).then(|| format!("{}-value", self.attrs.id()))
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
    fn a_disclosure_that_loads_fetches_with_a_constant_expression() {
        let html = Disclosure::new("files")
            .loads("/jobs/1/files", "Reading…")
            .id("files-1")
            .to_string();
        assert!(html.contains("data-url=\"/jobs/1/files\""), "{html}");
        assert!(
            html.contains("data-on:toggle=\"el.open &amp;&amp; @get(el.dataset.url)\"")
                && html.contains("data-init=\"el.open &amp;&amp; @get(el.dataset.url)\""),
            "{html}"
        );
    }

    #[test]
    fn a_fill_replaces_the_body_alone() {
        let patch = format!("{:?}", Disclosure::fill("files-1", "two files"));
        assert!(patch.contains("#files-1 > div"), "{patch}");
        assert!(
            patch.contains("Replace"),
            "a morph would leave a disclosure that ignores morphs alone: {patch}"
        );
        assert!(
            !patch.contains("details"),
            "the element and its loads stay: {patch}"
        );
    }

    #[test]
    fn a_disclosure_takes_its_summary_from_the_block() {
        let html = Disclosure::new("")
            .wrap(Markup::kit(
                "\n  <summary class=\"flex\"><b>title</b></summary>\n  <p>tags</p>".into(),
            ))
            .to_string();
        assert!(
            html.contains("<summary class=\"flex\"><b>title</b></summary>"),
            "{html}"
        );
        assert_eq!(html.matches("<summary").count(), 1, "{html}");
        assert!(html.contains("<div><p>tags</p></div>"), "{html}");
        let plain = Disclosure::new("files")
            .wrap(Markup::kit("<p>x</p>".into()))
            .to_string();
        assert!(plain.contains("<summary>files</summary>"), "{plain}");
    }

    #[test]
    fn a_loading_disclosure_shows_the_enso_until_answered() {
        let html = Disclosure::new("files")
            .loads("/f", "Reading…")
            .id("f")
            .to_string();
        assert!(
            html.contains("<div aria-busy=\"true\">Reading…</div>"),
            "{html}"
        );
    }

    #[test]
    fn optional_pairs_and_rows_from_a_template() {
        let none: Option<String> = None;
        let html = Pairs::new()
            .pair("size", "4 GB")
            .optional("shared on", &none)
            .wrap(Markup::kit("<dt>linked from</dt><dd>a 1 · b 2</dd>".into()))
            .to_string();
        assert!(!html.contains("shared on"), "{html}");
        assert!(
            html.contains("<dd>4 GB</dd><dt>linked from</dt><dd>a 1 · b 2</dd>"),
            "{html}"
        );
    }

    #[test]
    fn a_copy_field_names_its_input() {
        let html = CopyField::new("mcp url", "https://x", "copy")
            .id("mcp")
            .name("url")
            .icon_only()
            .to_string();
        assert!(html.contains("id=\"mcp-value\" name=\"url\""), "{html}");
        assert!(html.contains("aria-label=\"copy\""), "{html}");
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

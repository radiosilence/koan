//! Content in the kit's shapes: labelled [`Pairs`], a bulleted [`List`], a [`Disclosure`] that opens, a [`CopyField`] to
//! take a value from, and a region that [`scrolls_into_view`] as it is drawn.

use std::fmt;

use askama::Template;

use crate::arg::{Content, Flag, Markup, Text};
use crate::attrs::{Attrs, component, css_ident, render};
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
    given: Vec<(Text<'a>, Content<'a>)>,
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
        self.given.push((label.into(), value.into()));
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

    /// Its rows closer together, for a dense listing such as a release's details beside its tracks. A list of `rows`
    /// takes the same density from `class="rows tight"`.
    pub fn tight(mut self) -> Self {
        self.attrs.add_class("tight");
        self
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

/// Values in a bulleted list, each wrapped anywhere: `kit::List::new(scopes)`.
///
/// For values someone else chose, such as the hosts and scopes a consent screen asks about: the marker is what shows
/// where one value ends and the next begins, so a crafted value cannot pass as two, and a value with no spaces wraps
/// rather than running off the page. Items a template builds with a loop are the markup of a
/// `{% filter kit::wrap(list) %}` block, `<li>` elements, after those given. With nothing in it, it draws its
/// [`empty`](Self::empty) words, if given, as a compact [`Empty`](crate::Empty).
#[must_use = "a list draws nothing until it is rendered"]
#[derive(Clone, Debug, Default, Template)]
#[template(path = "list.html")]
pub struct List<'a> {
    items: Vec<Text<'a>>,
    rows: Markup,
    strong: bool,
    empty: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> List<'a> {
    /// `items`, in order.
    pub fn new<T: Into<Text<'a>>>(items: impl IntoIterator<Item = T>) -> Self {
        Self {
            items: items.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Each item in the strong ink, for the values the reader must not miss: where access goes.
    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }

    /// What it says when it holds nothing: `"no scopes"`.
    pub fn empty(mut self, words: impl Into<Text<'a>>) -> Self {
        self.empty = words.into();
        self
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty() && self.rows.as_str().trim().is_empty()
    }
}

impl Wrap for List<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.rows = content;
        self
    }
}

component!(List);

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
        render(&DisclosureBody { body: body.into() })
            .patch()
            .selector(format!("#{} > div", css_ident(id)))
            .mode(askama_datastar::ElementPatchMode::Replace)
    }
}

/// A disclosure's body alone, as [`Disclosure::fill`] sends it.
#[derive(Template)]
#[template(path = "disclosure_body.html")]
struct DisclosureBody<'c> {
    body: Content<'c>,
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
///
/// A [`description`](Self::description) says what the value is for and where to paste it, above the field, which it
/// describes as a field's note does.
#[must_use = "a copy field draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "copy.html")]
pub struct CopyField<'a> {
    label: Text<'a>,
    value: Text<'a>,
    button: Text<'a>,
    copied: Text<'a>,
    description: Text<'a>,
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
            description: Text::default(),
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

    /// What the value is for and where it goes, above the field: `"Paste this URL under Settings → Connectors."`. The
    /// field is described by it, so a screen reader reads it with the value.
    pub fn description(mut self, words: impl Into<Text<'a>>) -> Self {
        self.description = words.into();
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

    /// The description's id: built on the component's id, or else on what it copies, so two on a page differ.
    fn description_id(&self) -> String {
        if self.attrs.id().is_empty() {
            let mut hash = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&(self.label.as_str(), self.value.as_str()), &mut hash);
            format!("kit-copy-{:x}", std::hash::Hasher::finish(&hash))
        } else {
            format!("{}-description", self.attrs.id())
        }
    }
}

component!(CopyField);

/// The attribute that scrolls a region into view as the server draws it, such as a preview drawn in answer to a
/// request lower on the page: `<div id="preview" class="panel"{{ kit::scrolls_into_view() }}>`.
///
/// It runs as the element arrives, or as the attribute does when a patch morphs it into an element already there, and
/// brings the region's top into view.
#[must_use]
pub fn scrolls_into_view() -> ScrollsIntoView {
    ScrollsIntoView
}

/// What [`scrolls_into_view`] writes into a tag.
#[derive(Clone, Copy, Debug)]
pub struct ScrollsIntoView;

impl fmt::Display for ScrollsIntoView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(" data-init=\"el.scrollIntoView({block: 'start'})\"")
    }
}

impl askama::filters::HtmlSafe for ScrollsIntoView {}

#[cfg(test)]
mod tests {
    #[test]
    fn a_region_scrolls_into_view_as_it_is_drawn() {
        assert_eq!(
            super::scrolls_into_view().to_string(),
            " data-init=\"el.scrollIntoView({block: 'start'})\""
        );
    }

    #[test]
    fn tight_pairs_sit_closer() {
        let html = super::Pairs::new().pair("year", "1994").tight().to_string();
        assert!(html.starts_with("<dl class=\"pairs tight\">"), "{html}");
    }

    use super::*;

    #[test]
    fn pairs_take_words_and_markup() {
        let pairs: Pairs<'_> = [
            ("size", Content::from("4 GB")),
            ("source", Markup::kit("<a href=\"/x\">x</a>".into()).into()),
        ]
        .into_iter()
        .collect();
        let html = pairs.to_string();
        assert!(html.contains("<dt>size</dt><dd>4 GB</dd>"), "{html}");
        assert!(html.contains("<dd><a href=\"/x\">x</a></dd>"), "{html}");
    }

    #[test]
    fn a_list_marks_where_each_value_ends() {
        let html = List::new(["mail.read", "a<b"]).strong().to_string();
        assert_eq!(
            html,
            "<ul class=\"list\"><li><strong>mail.read</strong></li><li><strong>a&#60;b</strong></li></ul>"
        );
        let none: Vec<String> = Vec::new();
        let empty = List::new(none).empty("no scopes").to_string();
        assert_eq!(
            empty,
            "<div class=\"empty compact\"><strong>no scopes</strong></div>"
        );
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
        let odd = format!("{:?}", Disclosure::fill("a, #b", "x"));
        assert!(
            odd.contains("#a\\\\,\\\\ \\\\#b > div"),
            "an id cannot retarget the patch: {odd}"
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
    fn a_copy_field_is_described_by_its_description() {
        let html = CopyField::new("connector url", "https://x", "copy")
            .id("connector")
            .description("Paste it under Settings → Connectors.")
            .to_string();
        assert!(
            html.contains("<p id=\"connector-description\" class=\"description\">Paste it under"),
            "{html}"
        );
        assert!(
            html.contains("aria-describedby=\"connector-description\""),
            "{html}"
        );
        let anonymous = CopyField::new("a", "b", "copy")
            .description("c")
            .to_string();
        let id = CopyField::new("a", "b", "copy").description_id();
        assert!(
            anonymous.contains(&format!("aria-describedby=\"{id}\"")),
            "{anonymous}"
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

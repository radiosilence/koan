//! What every page carries around its content: the [`Head`] inside the app's `<head>`, an [`ImportMap`] for its
//! scripts, the [`SkipLink`] that opens its body, the [`TopBar`] over it, and the [`Footer`] under it with the
//! [`Release`] link.
//!
//! askama resolves `{% extends %}` against the app's own templates, so an app keeps its own base template and places
//! these in it.

use std::fmt;

use askama::Template;
use askama::filters::HtmlSafe;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::arg::{Content, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::font;
use crate::link::Link;
use crate::slot::Wrap;

/// The page's `<head>` contents, before the app's own: `<head>{{ head }}</head>`.
///
/// It writes the charset first, within the bytes a browser sniffs; the viewport, reaching under a phone's notch and
/// home indicator; both colour schemes, with the browser's chrome in the kit's ground for each; the font's preload,
/// at the URL the stylesheet asks for it by; the stylesheet; the import map; and Datastar.
#[must_use = "a head draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "head.html")]
pub struct Head<'a> {
    stylesheet: Text<'a>,
    title: Text<'a>,
    datastar: Option<String>,
    import_map: Option<ImportMap>,
    noindex: bool,
}

impl<'a> Head<'a> {
    /// The head for a page styled by the compiled stylesheet at `stylesheet`, as the page links it (with its `?v=`).
    /// The font is preloaded from beside it, where [`font::serve`] answers.
    pub fn new(stylesheet: impl Into<Text<'a>>) -> Self {
        Self {
            stylesheet: stylesheet.into(),
            title: Text::default(),
            datastar: None,
            import_map: None,
            noindex: false,
        }
    }

    /// The page's title.
    pub fn title(mut self, title: impl Into<Text<'a>>) -> Self {
        self.title = title.into();
        self
    }

    /// Datastar's script, from `path`, where the app routes `askama_datastar::script::serve`. The page's
    /// `script-src` needs `'unsafe-eval'`, since Datastar compiles its expressions with `Function`.
    pub fn datastar(mut self, path: &str) -> Self {
        self.datastar = Some(askama_datastar::script::url(path));
        self
    }

    /// An import map for the page's scripts, written as [`ImportMap::csp_source`] hashed it.
    pub fn import_map(mut self, map: &ImportMap) -> Self {
        self.import_map = Some(map.clone());
        self
    }

    /// Kept out of search engines: a signed-in page, a sign-in page, a share link.
    pub fn noindex(mut self) -> Self {
        self.noindex = true;
        self
    }

    /// The head's markup.
    #[must_use]
    pub fn html(&self) -> askama_datastar::Html {
        crate::attrs::render(self)
    }

    fn font(&self) -> String {
        font::url(self.stylesheet.as_str())
    }
}

impl HtmlSafe for Head<'_> {}

/// An inline import map, and the CSP source that lets it through: `script-src 'self' {{ map.csp_source() }}`.
///
/// The hash is of the exact text [`Head`] writes, so the two cannot drift: the map is built once, its JSON fixed, and
/// both read from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportMap {
    json: String,
    csp: String,
}

impl ImportMap {
    /// A map from each bare specifier to its versioned URL: `[("koan/player", "/assets/player.js?v=…")]`.
    pub fn new<'e>(entries: impl IntoIterator<Item = (&'e str, &'e str)>) -> Self {
        let imports: serde_json::Map<String, serde_json::Value> = entries
            .into_iter()
            .map(|(specifier, url)| (specifier.to_owned(), url.into()))
            .collect();
        // `<` escaped, so no URL can close the script element the map sits in.
        let json = serde_json::json!({ "imports": imports })
            .to_string()
            .replace('<', "\\u003c");
        let csp = format!(
            "'sha256-{}'",
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(json.as_bytes()))
        );
        Self { json, csp }
    }

    /// The map's text, as the page carries it.
    #[must_use]
    pub fn json(&self) -> &str {
        &self.json
    }

    /// The source a `script-src` names to let the map through: `'sha256-…'`.
    #[must_use]
    pub fn csp_source(&self) -> &str {
        &self.csp
    }
}

/// The map's JSON, unescaped: it is the program's own, and escaping it would change the bytes its hash is of.
struct MapJson<'m>(&'m str);

impl fmt::Display for MapJson<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl HtmlSafe for MapJson<'_> {}

impl Head<'_> {
    fn map(&self) -> Option<MapJson<'_>> {
        self.import_map.as_ref().map(|map| MapJson(&map.json))
    }
}

/// The link a keyboard reaches first, past the page's chrome to its content: shown only while focused.
#[must_use = "a skip link draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "skip.html")]
pub struct SkipLink<'a> {
    label: Text<'a>,
    target: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> SkipLink<'a> {
    /// A link saying `label`, to the element with the id `target`: the page's `<main>`.
    pub fn new(label: impl Into<Text<'a>>, target: impl Into<Text<'a>>) -> Self {
        Self {
            label: label.into(),
            target: target.into(),
            attrs: Attrs::default(),
        }
    }
}

component!(SkipLink);

/// The link to the release a page was built from, as a footer shows it: `slsk 0.25.0`.
#[must_use = "a release link draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "release.html")]
pub struct Release<'a> {
    app: Text<'a>,
    version: Text<'a>,
    repository: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Release<'a> {
    /// `app` at `version`, linking to its release at `repository` (`https://github.com/radiosilence/slsk-mcp`), whose
    /// tags are `v` and the version.
    pub fn new(
        app: impl Into<Text<'a>>,
        version: impl Into<Text<'a>>,
        repository: impl Into<Text<'a>>,
    ) -> Self {
        Self {
            app: app.into(),
            version: version.into(),
            repository: repository.into(),
            attrs: Attrs::default(),
        }
    }

    fn href(&self) -> String {
        format!(
            "{}/releases/tag/v{}",
            self.repository.as_str().trim_end_matches('/'),
            self.version
        )
    }
}

component!(Release);

/// The app's name as its mark, leading home: `kit::Wordmark::new("dither", "/")`. The [`TopBar`] leads with one; a page
/// without the bar, such as a sign-in page with the name above its form, places one itself.
#[must_use = "a wordmark draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "wordmark.html")]
pub struct Wordmark<'a> {
    name: Text<'a>,
    home: Text<'a>,
    home_label: Text<'a>,
    in_place: bool,
    attrs: Attrs<'a>,
}

impl<'a> Wordmark<'a> {
    /// The app's `name`, linking to `home`.
    pub fn new(name: impl Into<Text<'a>>, home: impl Into<Text<'a>>) -> Self {
        Self {
            name: name.into(),
            home: home.into(),
            home_label: Text::default(),
            in_place: false,
            attrs: Attrs::default(),
        }
    }

    /// What its link is named for a screen reader, when the name alone does not say where it goes:
    /// `"dither: your albums"`.
    pub fn home_label(mut self, label: impl Into<Text<'a>>) -> Self {
        self.home_label = label.into();
        self
    }

    /// Loads home in place, as [`Link::in_place`](crate::Link::in_place) does, for an app whose other links do: with a
    /// [`Navigation`](crate::Navigation) on the page, the link home is one of its links.
    pub fn in_place(mut self) -> Self {
        self.in_place = true;
        self
    }

    fn link(&self) -> Link<'_> {
        let mut link = Link::new(self.name.as_str(), self.home.as_str());
        link.attrs = self.attrs.clone();
        let link = link.class("wordmark");
        let link = match self.home_label.present() {
            Some(label) => link.attr("aria-label", label),
            None => link,
        };
        if self.in_place { link.in_place() } else { link }
    }
}

component!(Wordmark);

/// The bar along the top of an app's pages: its name, leading home, and its navigation and actions at the far end.
///
/// What follows the name is the markup of a `{% filter kit::wrap(bar) %}` block: links and buttons, laid out in a row.
/// It stays at the top as the page scrolls, clear of a phone's notch.
#[must_use = "a top bar draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "topbar.html")]
pub struct TopBar<'a> {
    wordmark: Wordmark<'a>,
    items: Markup,
    attrs: Attrs<'a>,
}

impl<'a> TopBar<'a> {
    /// The app's `name`, its wordmark, linking to `home`.
    pub fn new(name: impl Into<Text<'a>>, home: impl Into<Text<'a>>) -> Self {
        Self {
            wordmark: Wordmark::new(name, home),
            items: Markup::default(),
            attrs: Attrs::default(),
        }
    }

    /// What the wordmark's link is named for a screen reader, when the name alone does not say where it goes:
    /// `"dither: your albums"`.
    pub fn home_label(mut self, label: impl Into<Text<'a>>) -> Self {
        self.wordmark = self.wordmark.home_label(label);
        self
    }

    /// Loads home in place; see [`Wordmark::in_place`].
    pub fn in_place(mut self) -> Self {
        self.wordmark = self.wordmark.in_place();
        self
    }
}

impl Wrap for TopBar<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.items = content;
        self
    }
}

component!(TopBar);

/// The foot of an app's pages: what it holds, centred, and padded clear of a phone's home indicator.
///
/// Its content is [`body`](Self::body), or the markup of a `{% filter kit::wrap(footer) %}` block, usually the
/// [`Release`]: `kit::Footer::new().body(kit::Release::new(…))`.
#[must_use = "a footer draws nothing until it is rendered"]
#[derive(Clone, Debug, Default, Template)]
#[template(path = "footer.html")]
pub struct Footer<'a> {
    body: Content<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Footer<'a> {
    /// An empty footer.
    pub fn new() -> Self {
        Self::default()
    }

    /// What it holds.
    pub fn body(mut self, body: impl Into<Content<'a>>) -> Self {
        self.body = body.into();
        self
    }
}

impl Wrap for Footer<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.body = content.into();
        self
    }
}

component!(Footer);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_import_map_is_let_through_by_its_own_hash() {
        let map = ImportMap::new([("koan/player", "/assets/player.js?v=1</script>")]);
        let head = Head::new("/assets/app.css").import_map(&map).to_string();
        let (_, rest) = head
            .split_once("<script type=\"importmap\">")
            .expect("the head carries the map");
        let (json, _) = rest
            .split_once("</script>")
            .expect("the map's script is closed");
        let digest =
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(json.as_bytes()));
        assert_eq!(
            map.csp_source(),
            format!("'sha256-{digest}'"),
            "the CSP must hash the bytes the page carries"
        );
        assert!(
            !json.contains("</script>"),
            "a URL cannot close the map's script: {json}"
        );
    }

    #[test]
    fn the_head_preloads_the_font_the_stylesheet_asks_for() {
        let head = Head::new("/assets/app.css?v=abc")
            .datastar("/assets/datastar.js")
            .title("jobs · slsk")
            .to_string();
        assert!(
            head.starts_with("<meta charset=\"utf-8\">"),
            "the charset comes first: {head}"
        );
        assert!(
            head.contains(&format!(
                "<link rel=\"preload\" href=\"{}\"",
                font::url("/assets/app.css")
            )),
            "{head}"
        );
        assert!(
            head.contains("<script type=\"module\" src=\"/assets/datastar.js?v="),
            "{head}"
        );
        assert!(head.contains("<title>jobs · slsk</title>"), "{head}");
    }

    #[test]
    fn the_browser_chrome_is_the_kits_ground() {
        let tokens = include_str!("../tokens.css");
        let head = Head::new("app.css").to_string();
        for (scheme, ground) in [("dark", "#1e1e1e"), ("light", "#ffffff")] {
            assert!(
                tokens.contains(&format!("--bg: {ground};")),
                "tokens.css's {scheme} ground is no longer {ground}"
            );
            assert!(
                head.contains(&format!(
                    "content=\"{ground}\" media=\"(prefers-color-scheme: {scheme})\""
                )),
                "{head}"
            );
        }
    }

    #[test]
    fn a_top_bar_leads_home_and_holds_the_rest() {
        let bar = TopBar::new("dither", "/")
            .home_label("dither: your albums")
            .wrap(Markup::kit("<a href=\"/account\">jc</a>".into()))
            .to_string();
        assert!(bar.starts_with("<header class=\"topbar\">"), "{bar}");
        assert!(
            bar.contains(
                "<a class=\"wordmark\" href=\"/\" aria-label=\"dither: your albums\">dither</a>"
            ),
            "{bar}"
        );
        assert!(bar.contains("<a href=\"/account\">jc</a>"), "{bar}");
    }

    #[test]
    fn a_footer_holds_the_release() {
        let footer = Footer::new()
            .body(Release::new("slsk", "1.0.0", "https://x"))
            .to_string();
        assert!(footer.starts_with("<footer class=\"footer\">"), "{footer}");
        assert!(footer.contains("class=\"release\""), "{footer}");
    }

    #[test]
    fn a_release_links_its_tag() {
        let link = Release::new(
            "slsk",
            "0.25.0",
            "https://github.com/radiosilence/slsk-mcp/",
        )
        .to_string();
        assert!(
            link.contains("href=\"https://github.com/radiosilence/slsk-mcp/releases/tag/v0.25.0\""),
            "{link}"
        );
        assert!(link.contains(">slsk 0.25.0<"), "{link}");
    }
}

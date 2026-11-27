//! What every page carries around its content: the [`Head`] inside the app's `<head>`, an [`ImportMap`] for its
//! scripts, the [`SkipLink`] that opens its body, and the [`Release`] link its footer shows.
//!
//! askama resolves `{% extends %}` against the app's own templates, so an app keeps its own base template and places
//! these in it.

use std::fmt;

use askama::Template;
use askama::filters::HtmlSafe;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::arg::Text;
use crate::attrs::{Attrs, component};
use crate::font;

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

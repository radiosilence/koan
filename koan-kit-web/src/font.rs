//! Geist Mono, the kit's one typeface, served from the binary.
//!
//! The kit's stylesheet names the font relative to itself, as `geist-mono.woff2?v=` and a hash of the font, so an app
//! routes [`serve`] beside its compiled stylesheet: `/assets/geist-mono.woff2` for `/assets/app.css`. Asked for by that
//! URL the font is kept for good, and a new font arrives under a new one; [`Head`](crate::Head) preloads the same URL,
//! so the preload is the request the stylesheet makes.

use std::sync::LazyLock;

use axum::extract::RawQuery;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::action::fnv1a;

/// The font file's name, as the stylesheet asks for it.
pub const FILE: &str = "geist-mono.woff2";

/// The font: Geist Mono, a variable weight from 100 to 900, under the SIL Open Font License (`OFL.txt`).
pub const BYTES: &[u8] = include_bytes!("../geist-mono.woff2");

/// `v=` and a hash of [`BYTES`]: the query the stylesheet names the font with. `tokens.css` carries it too, and a test
/// fails when the two disagree.
static QUERY: LazyLock<String> = LazyLock::new(|| format!("v={:016x}", fnv1a(BYTES)));

/// The font's URL beside the stylesheet at `stylesheet`, as the stylesheet resolves it: `/assets/geist-mono.woff2?v=…`
/// for `/assets/app.css?v=…`.
#[must_use]
pub fn url(stylesheet: &str) -> String {
    let path = stylesheet.split(['?', '#']).next().unwrap_or_default();
    let directory = path.rsplit_once('/').map_or("", |(directory, _)| directory);
    let slash = if path.contains('/') { "/" } else { "" };
    format!("{directory}{slash}{FILE}?{}", *QUERY)
}

/// The font, as a handler for the route beside the app's stylesheet.
///
/// Asked for by the URL the stylesheet names it with, it is cached for a year as immutable; any other way, for an
/// hour, since that address will not always be this font.
#[expect(
    clippy::unused_async,
    reason = "axum takes a handler as an async fn, and this one has nothing to wait for"
)]
pub async fn serve(RawQuery(query): RawQuery) -> Response {
    let cache = if query.as_deref() == Some(QUERY.as_str()) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    };
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, cache),
        ],
        BYTES,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stylesheet_names_this_font() {
        let tokens = include_str!("../tokens.css");
        let named = format!("url(\"{FILE}?{}\")", *QUERY);
        assert!(
            tokens.contains(&named),
            "tokens.css must name the font as {named}, the hash of geist-mono.woff2, so a new font is fetched again"
        );
    }

    #[test]
    fn the_font_sits_beside_the_stylesheet() {
        assert_eq!(
            url("/assets/app.css?v=1"),
            format!("/assets/{FILE}?{}", *QUERY)
        );
        assert_eq!(
            url("/ui/assets/ui.css"),
            format!("/ui/assets/{FILE}?{}", *QUERY)
        );
        assert_eq!(url("out.css"), format!("{FILE}?{}", *QUERY));
        assert_eq!(url("/app.css"), format!("/{FILE}?{}", *QUERY));
    }
}

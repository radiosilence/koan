//! Markup the server built, as a type: a template writes an [`Html`] as it is
//! and escapes everything else, so whether a value is safe to splice in is
//! decided where it is made rather than by a `|safe` in the template.

use std::fmt;
use std::ops::Deref;

/// HTML rendered from a template or written by this crate's own code, never
/// text that came from anyone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Html(String);

/// No markup, for a borrowed [`Html`] that is absent.
pub(crate) static NOTHING: Html = Html(String::new());

impl Html {
    /// Markup this crate wrote itself, such as an icon or a QR code. Nothing
    /// a person typed, a tag or a name may reach it unescaped.
    pub(crate) fn trusted(markup: impl Into<String>) -> Self {
        Self(markup.into())
    }
}

impl FromIterator<Html> for Html {
    fn from_iter<I: IntoIterator<Item = Html>>(parts: I) -> Self {
        Self(parts.into_iter().map(|h| h.0).collect())
    }
}

impl Deref for Html {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Html {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl askama::filters::HtmlSafe for Html {}

impl From<Html> for String {
    fn from(h: Html) -> Self {
        h.0
    }
}

/// Rendering a template. askama fails only when a value's `Display` does,
/// which for the text, numbers and [`Html`] these templates hold is a bug, so
/// it panics. The web UI and share routes sit behind `CatchPanicLayer`, which
/// answers 500 rather than a page or an event drawn as nothing; a render in an
/// SSE stream that has already begun ends the stream instead.
pub(crate) trait Render {
    fn html(&self) -> Html;
}

impl<T: askama::Template> Render for T {
    fn html(&self) -> Html {
        match self.render() {
            Ok(s) => Html(s),
            Err(e) => panic!("rendering {}: {e}", std::any::type_name::<T>()),
        }
    }
}

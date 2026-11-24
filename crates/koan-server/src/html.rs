//! Markup the server built, as a type: a template writes an [`Html`] as it is
//! and escapes everything else, so whether a value is safe to splice in is
//! decided where it is made rather than by a `|safe` in the template. The type
//! is `askama_datastar`'s, which also makes it a Datastar patch; how a failed
//! render is handled is koan's, below.

pub(crate) use askama_datastar::Html;

/// No markup, for a borrowed [`Html`] that is absent.
pub(crate) static NOTHING: Html = Html::new();

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
        match Html::render(self) {
            Ok(html) => html,
            Err(e) => panic!("rendering {}: {e}", std::any::type_name::<T>()),
        }
    }
}

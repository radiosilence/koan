//! Children for a component, written in the template that places it.
//!
//! askama resolves `{% include %}`, `{% import %}` and `{% extends %}` against the app's own template directories,
//! so a crate's templates cannot hold an app's markup. A filter block can: askama renders the block's markup, escaping
//! its values as everywhere, and hands it to the filter marked safe. `kit::wrap` takes it and puts it inside the
//! component it is given:
//!
//! ```html
//! {% filter kit::wrap(kit::Sheet::new("add", "add a download").close("close")) %}
//!   <form action="/downloads" method="post">…</form>
//! {% endfilter %}
//! ```
//!
//! Only a filter block's content, or a value a template has marked `|safe`, can arrive here: what the filter takes is
//! askama's `Safe`, so a plain string cannot be passed off as markup.

use std::fmt::Display;

use askama::filters::{HtmlSafe, Safe};

use crate::arg::Markup;

/// A component with a slot for markup: a sheet's body, a disclosure's, a notice's words, a field's control.
pub trait Wrap {
    /// The component holding `content`.
    #[must_use]
    fn wrap(self, content: Markup) -> Self;
}

pub use filter::wrap;

#[expect(
    missing_docs,
    clippy::missing_errors_doc,
    clippy::inline_always,
    reason = "askama's filter_fn writes the filter as a struct and a builder, with no docs of its own"
)]
mod filter {
    use super::{Display, HtmlSafe, Markup, Safe, Wrap};

    /// The filter: `{% filter kit::wrap(component) %}…{% endfilter %}` draws the component with the block inside it.
    ///
    /// # Errors
    ///
    /// None of its own: it fails only where rendering the block's content does.
    #[askama::filter_fn]
    pub fn wrap<T: Display, C: Wrap + HtmlSafe>(
        content: &Safe<T>,
        _: &dyn askama::Values,
        component: C,
    ) -> askama::Result<C> {
        Ok(component.wrap(Markup::kit(content.0.to_string())))
    }
}

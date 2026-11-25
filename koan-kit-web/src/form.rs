//! A form's own attributes, for the `<form>` an app writes around its fields: `<form {{ form.attrs() }}>`.
//!
//! A form keeps its real `method` and `action`, so a submit made before Datastar loads goes as a POST with its fields
//! in the body, never in a query string where a password would reach the logs. That submit is not Datastar's, so
//! `require_datastar_on_post` refuses it, which is how it should fail. A form that must work without script, such as
//! sign-in or consent, is a [`Form::plain`], posting to a route outside that layer which checks `Sec-Fetch-Site` or
//! that `Origin` matches `Host` (§8), and answers with a redirect.

use std::fmt;

use askama::filters::HtmlSafe;

use crate::action::{Busy, Method};
use crate::arg::Text;
use crate::attrs::{Attrs, attribute};
use crate::button::Button;

/// A form's attributes and its submit:
///
/// ```html
/// {% let form = kit::Form::post(save) %}
/// <form {{ form.attrs() }} class="grid gap-4">
///   {{ kit::Field::text("name", "name").required() }}
///   {{ form.submit(kit::Button::primary("save")) }}
/// </form>
/// ```
#[must_use = "a form's attributes go in its <form> tag"]
#[derive(Clone, Debug)]
pub struct Form<'a> {
    action: Text<'a>,
    method: Method,
    datastar: bool,
    live: bool,
    attrs: Attrs<'a>,
}

impl<'a> Form<'a> {
    fn of(method: Method, action: Text<'a>, datastar: bool) -> Self {
        Self {
            action,
            method,
            datastar,
            live: false,
            attrs: Attrs::default(),
        }
    }

    /// Posts its fields to `action` through Datastar, its submit busy while the request runs.
    pub fn post(action: impl Into<Text<'a>>) -> Self {
        Self::of(Method::Post, action.into(), true)
    }

    /// Reads `action` with its fields as the query, through Datastar: a search, a filter.
    pub fn get(action: impl Into<Text<'a>>) -> Self {
        Self::of(Method::Get, action.into(), true)
    }

    /// Posts to `action` as a browser does, with no script: sign-in, consent, anything that must work before
    /// Datastar loads. Its route sits outside `require_datastar_on_post` and checks the request's origin itself.
    pub fn plain(action: impl Into<Text<'a>>) -> Self {
        Self::of(Method::Post, action.into(), false)
    }

    /// Sends itself as the person types, a third of a second after they pause: a search that answers as it goes.
    pub fn live(mut self) -> Self {
        self.live = true;
        self
    }

    /// The form's id.
    pub fn id(mut self, id: impl Into<Text<'a>>) -> Self {
        self.attrs.set_id(id.into());
        self
    }

    /// A `data-{key}` attribute; see [`Button::data`](crate::Button::data).
    ///
    /// # Panics
    ///
    /// When `key` is not lowercase letters, digits and hyphens, or names a Datastar attribute.
    pub fn data(mut self, key: &'static str, value: impl Into<Text<'a>>) -> Self {
        self.attrs.add_data(key, value.into());
        self
    }

    /// A Datastar attribute the kit does not write itself; see [`Button::datastar`](crate::Button::datastar).
    ///
    /// # Panics
    ///
    /// When `name` holds anything an attribute name cannot.
    pub fn datastar(mut self, name: &'static str, expression: &'static str) -> Self {
        self.attrs.add_datastar(name, expression);
        self
    }

    /// The attributes, for the app's `<form>` tag: `<form {{ form.attrs() }} class="grid gap-4">`.
    #[must_use]
    pub fn attrs(&self) -> FormAttrs<'_, 'a> {
        FormAttrs(self)
    }

    /// `button` as the form's submit, busy while the form's request runs: `form.submit(kit::Button::primary("save"))`.
    pub fn submit(&self, button: Button<'a>) -> Button<'a> {
        if self.datastar {
            button.submits_with(self.busy())
        } else {
            button.submit()
        }
    }

    fn busy(&self) -> Busy {
        Busy::of(self.method, self.action.as_str())
    }
}

/// A [`Form`]'s attributes, written into the app's `<form>` tag.
pub struct FormAttrs<'f, 'a>(&'f Form<'a>);

impl fmt::Display for FormAttrs<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let form = self.0;
        attribute(f, "action", form.action.as_str())?;
        write!(f, " method=\"{}\"", form.method.form())?;
        if form.datastar {
            let request = format!(
                "{}(el.getAttribute('action'), {{contentType: 'form'}})",
                form.method.action()
            );
            write!(
                f,
                " data-indicator=\"{}\" data-on:submit=\"{request}\"",
                form.busy()
            )?;
            if form.live {
                write!(f, " data-on:input__debounce.300ms=\"{request}\"")?;
            }
        }
        write!(f, "{}", form.attrs)
    }
}

impl HtmlSafe for FormAttrs<'_, '_> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_keeps_its_method_and_action_and_posts_through_datastar() {
        let form = Form::post("/settings");
        let attrs = form.attrs().to_string();
        assert!(
            attrs.starts_with(" action=\"/settings\" method=\"post\""),
            "{attrs}"
        );
        assert!(
            attrs.contains(
                "data-on:submit=\"@post(el.getAttribute('action'), {contentType: 'form'})\""
            ),
            "{attrs}"
        );
        let submit = form.submit(Button::primary("save")).to_string();
        let busy = Busy::of(Method::Post, "/settings");
        assert!(
            attrs.contains(&format!("data-indicator=\"{busy}\"")),
            "{attrs}"
        );
        assert!(
            submit.contains(&format!("data-class:busy=\"${busy}\"")),
            "the submit is busy by the form's request: {submit}"
        );
        assert!(submit.contains("type=\"submit\""), "{submit}");
    }

    #[test]
    fn a_plain_form_needs_no_script() {
        let form = Form::plain("/login");
        assert_eq!(
            form.attrs().to_string(),
            " action=\"/login\" method=\"post\""
        );
        assert!(
            !form
                .submit(Button::primary("sign in"))
                .to_string()
                .contains("data-")
        );
    }
}

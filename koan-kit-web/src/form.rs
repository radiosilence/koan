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
use crate::field::Field;

/// A form's attributes, its fields and its submit:
///
/// ```html
/// {% let form = kit::Form::post(save).id(row.form_id()) %}
/// <form {{ form.attrs() }} class="grid gap-4">
///   {{ form.field(kit::Field::text("name", "name").required()) }}
///   {{ form.submit(kit::Button::primary("save")) }}
/// </form>
/// ```
///
/// A field placed through [`field`](Self::field) builds its ids on the form's, so a page holding the same form once per
/// row gives only each form an id.
#[must_use = "a form's attributes go in its <form> tag"]
#[derive(Clone, Debug)]
pub struct Form<'a> {
    action: Text<'a>,
    method: Method,
    datastar: bool,
    live: bool,
    busy_key: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Form<'a> {
    fn of(method: Method, action: Text<'a>, datastar: bool) -> Self {
        Self {
            action,
            method,
            datastar,
            live: false,
            busy_key: Text::default(),
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

    /// Reads `action` as a browser does, with its fields as the query and no script: a search that is a page of its
    /// own.
    pub fn plain_get(action: impl Into<Text<'a>>) -> Self {
        Self::of(Method::Get, action.into(), false)
    }

    /// Joins the busy group `key` names, as [`Button::busy_key`](crate::Button::busy_key) does: while any request of
    /// the group runs, its submit is disabled. Without one, its busy signal is named by its method, action and id, so
    /// forms posting one action row by row are told apart by their ids.
    pub fn busy_key(mut self, key: impl Into<Text<'a>>) -> Self {
        self.busy_key = key.into();
        self
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

    /// `field`, its ids built on this form's id when it has one and the field has none of its own.
    pub fn field(&self, field: Field<'a>) -> Field<'a> {
        match self.attrs.id() {
            "" => field,
            id => field.scope(Text::from(id.to_owned())),
        }
    }

    fn busy(&self) -> Busy {
        match self.busy_key.present() {
            Some(key) => Busy::keyed(key),
            None => Busy::own(&[
                "form",
                self.method.form(),
                self.action.as_str(),
                self.attrs.id(),
            ]),
        }
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
            // A form left invalid would start a request Datastar abandons unsettled, keeping its submit busy for good;
            // one marked novalidate is the server's to check, as Datastar's own form fetch has it.
            let options = match form.busy().options() {
                "" => String::new(),
                more => format!(", {more}"),
            };
            let request = format!(
                "(el.noValidate || el.checkValidity()) && {}(el.getAttribute('action'), {{contentType: 'form'{options}}})",
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
                "data-on:submit=\"(el.noValidate || el.checkValidity()) && @post(el.getAttribute('action'), {contentType: 'form'})\""
            ),
            "{attrs}"
        );
        let submit = form.submit(Button::primary("save")).to_string();
        let busy = form.busy();
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
    fn a_repeated_form_names_its_fields_by_its_own_id() {
        let form = Form::post("/mcp/credential").id("cred-github");
        let field = form
            .field(Field::text("value", "token").hint("Stored sealed."))
            .to_string();
        assert!(field.contains("id=\"cred-github-value-note\""), "{field}");
        assert!(
            field.contains("aria-describedby=\"cred-github-value-note\""),
            "{field}"
        );
        let own = form
            .field(Field::text("value", "token").hint("x").id("mine"))
            .to_string();
        assert!(
            own.contains("id=\"mine-note\""),
            "a field's own id wins: {own}"
        );
        let other = Form::post("/mcp/credential").id("cred-slack");
        assert_ne!(
            form.attrs().to_string(),
            other.attrs().to_string(),
            "two rows' forms are busy apart"
        );
        assert_eq!(
            form.attrs().to_string(),
            Form::post("/mcp/credential")
                .id("cred-github")
                .attrs()
                .to_string(),
            "and each draws the same markup every time"
        );
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

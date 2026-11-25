//! Controls, and a [`Field`]: a control with its label and a note under it.
//!
//! The controls are the kit's plain elements, [`Input`], [`Select`] and [`Textarea`], each usable alone (in a
//! [`Combo`], beside a button) or in a field. A field links its note to its control by `aria-describedby`, so a
//! screen reader reads the hint or the error with the control, and marks an error with `aria-invalid`.

use std::fmt;

use askama::Template;

use crate::arg::{Amount, AsText, Content, Flag, Markup, Text};
use crate::attrs::{Attrs, attribute, component, escape};
use crate::button::Button;
use crate::slot::Wrap;

/// What an [`Input`] takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputKind {
    /// A line of text.
    #[default]
    Text,
    /// A secret, hidden as it is typed.
    Password,
    /// An email address.
    Email,
    /// A URL.
    Url,
    /// Words to search for.
    Search,
    /// A telephone number.
    Tel,
    /// A number, between [`min`](Input::min) and [`max`](Input::max).
    Number,
    /// A date.
    Date,
    /// A time.
    Time,
    /// A date and a time, in the reader's zone.
    DateTime,
    /// A colour.
    Color,
    /// Files to upload.
    File,
    /// A value along a range: the kit's hairline slider.
    Range,
    /// A value the page carries but does not show.
    Hidden,
}

impl InputKind {
    fn attribute(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Password => "password",
            Self::Email => "email",
            Self::Url => "url",
            Self::Search => "search",
            Self::Tel => "tel",
            Self::Number => "number",
            Self::Date => "date",
            Self::Time => "time",
            Self::DateTime => "datetime-local",
            Self::Color => "color",
            Self::File => "file",
            Self::Range => "range",
            Self::Hidden => "hidden",
        }
    }
}

/// Validates a signal name a control binds, which becomes part of an attribute's value Datastar reads as a name.
pub(crate) fn signal_name(name: &'static str) -> &'static str {
    assert!(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.')),
        "{name:?} is not a signal name"
    );
    name
}

/// The props every control shares.
#[derive(Clone, Debug, Default)]
struct Common<'a> {
    name: Text<'a>,
    label: Text<'a>,
    required: bool,
    disabled: bool,
    autofocus: bool,
    bind: Option<&'static str>,
    attrs: Attrs<'a>,
}

impl Common<'_> {
    /// `name`, the state props, the binding and the aria the control's field gives it.
    fn write(&self, f: &mut fmt::Formatter<'_>, note: Option<&str>, invalid: bool) -> fmt::Result {
        if let Some(name) = self.name.present() {
            attribute(f, "name", name)?;
        }
        if let Some(label) = self.label.present() {
            attribute(f, "aria-label", label)?;
        }
        if let Some(note) = note {
            attribute(f, "aria-describedby", note)?;
        }
        if invalid {
            f.write_str(" aria-invalid=\"true\"")?;
        }
        if let Some(signal) = self.bind {
            write!(f, " data-bind=\"{signal}\"")?;
        }
        for (on, flag) in [
            (self.required, " required"),
            (self.disabled, " disabled"),
            (self.autofocus, " autofocus"),
        ] {
            if on {
                f.write_str(flag)?;
            }
        }
        write!(f, "{}", self.attrs)
    }

    fn class(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.attrs.classes().is_empty() {
            write!(
                f,
                " class=\"{}\"",
                self.attrs.classes().to_string().trim_start()
            )?;
        }
        Ok(())
    }
}

/// The props every control takes, generated for each.
macro_rules! control {
    ($control:ident) => {
        impl<'a> $control<'a> {
            /// Its name for a screen reader, when no field's label names it: in a [`Combo`], or alone.
            pub fn label(mut self, label: impl Into<Text<'a>>) -> Self {
                self.common.label = label.into();
                self
            }

            /// Must be filled before its form sends.
            pub fn required(mut self) -> Self {
                self.common.required = true;
                self
            }

            /// Unavailable, and drawn so.
            pub fn disabled(mut self, on: impl Into<Flag>) -> Self {
                self.common.disabled = on.into().get();
                self
            }

            /// Focused when its page or dialog opens.
            pub fn autofocus(mut self) -> Self {
                self.common.autofocus = true;
                self
            }

            /// Two-way bound to the Datastar signal `signal`. A signal starting `_` stays on the page; any other is
            /// sent with every request.
            ///
            /// # Panics
            ///
            /// When `signal` is not letters, digits, `_` and `.`.
            pub fn bind(mut self, signal: &'static str) -> Self {
                self.common.bind = Some(signal_name(signal));
                self
            }

            /// The control's id.
            pub fn id(mut self, id: impl Into<Text<'a>>) -> Self {
                self.common.attrs.set_id(id.into());
                self
            }

            /// Classes from the app's stylesheet for where it sits; see [`Button::class`](crate::Button::class).
            pub fn class(mut self, class: &'static str) -> Self {
                self.common.attrs.add_class(class);
                self
            }

            /// A `data-{key}` attribute; see [`Button::data`](crate::Button::data).
            ///
            /// # Panics
            ///
            /// When `key` is not lowercase letters, digits and hyphens, or names a Datastar attribute.
            pub fn data(mut self, key: &'static str, value: impl Into<Text<'a>>) -> Self {
                self.common.attrs.add_data(key, value.into());
                self
            }

            /// Another attribute; see [`Button::attr`](crate::Button::attr).
            ///
            /// # Panics
            ///
            /// When `name` is not an attribute name, or has a way in of its own.
            pub fn attr(mut self, name: &'static str, value: impl Into<Text<'a>>) -> Self {
                self.common.attrs.add_attr(name, value.into());
                self
            }

            /// A Datastar attribute the kit does not write itself; see [`Button::datastar`](crate::Button::datastar).
            ///
            /// # Panics
            ///
            /// When `name` holds anything an attribute name cannot.
            pub fn datastar(mut self, name: &'static str, expression: &'static str) -> Self {
                self.common.attrs.add_datastar(name, expression);
                self
            }

            /// The control's markup.
            pub fn html(&self) -> askama_datastar::Html {
                askama_datastar::Html::trusted(self.to_string())
            }
        }

        impl fmt::Display for $control<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.write(f, None, false)
            }
        }

        impl askama::filters::HtmlSafe for $control<'_> {}

        impl<'a> From<$control<'a>> for Control<'a> {
            fn from(control: $control<'a>) -> Self {
                Self::$control(control)
            }
        }

        impl From<$control<'_>> for Markup {
            fn from(control: $control<'_>) -> Self {
                Markup::kit(control.to_string())
            }
        }
    };
}

/// An `<input>`: `kit::Input::email("email").required()`.
#[must_use = "an input draws nothing until it is rendered"]
#[derive(Clone, Debug)]
pub struct Input<'a> {
    kind: InputKind,
    value: Text<'a>,
    placeholder: Text<'a>,
    autocomplete: Option<&'static str>,
    readonly: bool,
    bounds: [Option<Amount>; 3],
    common: Common<'a>,
}

impl<'a> Input<'a> {
    /// An input of `kind` named `name`.
    pub fn new(kind: InputKind, name: impl Into<Text<'a>>) -> Self {
        Self {
            kind,
            value: Text::default(),
            placeholder: Text::default(),
            autocomplete: None,
            readonly: false,
            bounds: [None; 3],
            common: Common {
                name: name.into(),
                ..Common::default()
            },
        }
    }

    /// A line of text.
    pub fn text(name: impl Into<Text<'a>>) -> Self {
        Self::new(InputKind::Text, name)
    }

    /// A secret, hidden as it is typed.
    pub fn password(name: impl Into<Text<'a>>) -> Self {
        Self::new(InputKind::Password, name)
    }

    /// An email address.
    pub fn email(name: impl Into<Text<'a>>) -> Self {
        Self::new(InputKind::Email, name)
    }

    /// A URL.
    pub fn url(name: impl Into<Text<'a>>) -> Self {
        Self::new(InputKind::Url, name)
    }

    /// Words to search for.
    pub fn search(name: impl Into<Text<'a>>) -> Self {
        Self::new(InputKind::Search, name)
    }

    /// A number.
    pub fn number(name: impl Into<Text<'a>>) -> Self {
        Self::new(InputKind::Number, name)
    }

    /// A value the page carries but does not show.
    pub fn hidden(name: impl Into<Text<'a>>, value: impl Into<Text<'a>>) -> Self {
        Self::new(InputKind::Hidden, name).value(value)
    }

    /// What it holds as the page opens: the value last saved, or what was sent and refused.
    pub fn value(mut self, value: impl Into<Text<'a>>) -> Self {
        self.value = value.into();
        self
    }

    /// An example of what goes in it, shown while it is empty.
    pub fn placeholder(mut self, placeholder: impl Into<Text<'a>>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// What a browser may fill it with: `"username"`, `"current-password"`, `"one-time-code"`, `"off"`.
    pub fn autocomplete(mut self, token: &'static str) -> Self {
        self.autocomplete = Some(token);
        self
    }

    /// Shown but not edited.
    pub fn readonly(mut self) -> Self {
        self.readonly = true;
        self
    }

    /// The least a number, date or range takes.
    pub fn min(mut self, min: impl Into<Amount>) -> Self {
        self.bounds[0] = Some(min.into());
        self
    }

    /// The most a number, date or range takes.
    pub fn max(mut self, max: impl Into<Amount>) -> Self {
        self.bounds[1] = Some(max.into());
        self
    }

    /// The steps a number or range moves in.
    pub fn step(mut self, step: impl Into<Amount>) -> Self {
        self.bounds[2] = Some(step.into());
        self
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, note: Option<&str>, invalid: bool) -> fmt::Result {
        write!(f, "<input type=\"{}\"", self.kind.attribute())?;
        self.common.class(f)?;
        if let Some(value) = self.value.present() {
            attribute(f, "value", value)?;
        }
        if let Some(placeholder) = self.placeholder.present() {
            attribute(f, "placeholder", placeholder)?;
        }
        if let Some(token) = self.autocomplete {
            attribute(f, "autocomplete", token)?;
        }
        for (name, bound) in ["min", "max", "step"].into_iter().zip(self.bounds) {
            if let Some(bound) = bound {
                attribute(f, name, &bound.to_string())?;
            }
        }
        if self.readonly {
            f.write_str(" readonly")?;
        }
        self.common.write(f, note, invalid)?;
        f.write_str(">")
    }
}

control!(Input);

/// One of a [`Select`]'s options: its value, and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice<'a> {
    value: Text<'a>,
    label: Text<'a>,
}

impl<'a> Choice<'a> {
    /// `value`, saying `label`.
    pub fn new(value: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// A value that says itself.
impl<'a, T: AsText + ?Sized> From<&'a T> for Choice<'a> {
    fn from(value: &'a T) -> Self {
        Self::new(value, value)
    }
}

impl<'a, V: Into<Text<'a>>, L: Into<Text<'a>>> From<(V, L)> for Choice<'a> {
    fn from((value, label): (V, L)) -> Self {
        Self::new(value, label)
    }
}

impl<'a> From<&'a (String, String)> for Choice<'a> {
    fn from((value, label): &'a (String, String)) -> Self {
        Self::new(value, label)
    }
}

impl<'a, 'b: 'a> From<&'a (&'b str, &'b str)> for Choice<'a> {
    fn from((value, label): &'a (&'b str, &'b str)) -> Self {
        Self::new(*value, *label)
    }
}

/// A `<select>`: `kit::Select::new("region").options(regions).selected(region)`.
#[must_use = "a select draws nothing until it is rendered"]
#[derive(Clone, Debug)]
pub struct Select<'a> {
    choices: Vec<Choice<'a>>,
    selected: Text<'a>,
    common: Common<'a>,
}

impl<'a> Select<'a> {
    /// A select named `name`, with no options yet.
    pub fn new(name: impl Into<Text<'a>>) -> Self {
        Self {
            choices: Vec::new(),
            selected: Text::default(),
            common: Common {
                name: name.into(),
                ..Common::default()
            },
        }
    }

    /// One option, after those already given.
    pub fn option(mut self, value: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        self.choices.push(Choice::new(value, label));
        self
    }

    /// Options, after those already given: `(value, label)` pairs, or values that say themselves.
    pub fn options<C: Into<Choice<'a>>>(mut self, choices: impl IntoIterator<Item = C>) -> Self {
        self.choices.extend(choices.into_iter().map(Into::into));
        self
    }

    /// The option chosen as the page opens.
    pub fn selected(mut self, value: impl Into<Text<'a>>) -> Self {
        self.selected = value.into();
        self
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, note: Option<&str>, invalid: bool) -> fmt::Result {
        f.write_str("<select")?;
        self.common.class(f)?;
        self.common.write(f, note, invalid)?;
        f.write_str(">")?;
        for choice in &self.choices {
            f.write_str("<option")?;
            attribute(f, "value", choice.value.as_str())?;
            if choice.value == self.selected {
                f.write_str(" selected")?;
            }
            f.write_str(">")?;
            escape(f, choice.label.as_str())?;
            f.write_str("</option>")?;
        }
        f.write_str("</select>")
    }
}

control!(Select);

/// A `<textarea>`: `kit::Textarea::new("notes").rows(4)`.
#[must_use = "a textarea draws nothing until it is rendered"]
#[derive(Clone, Debug)]
pub struct Textarea<'a> {
    value: Text<'a>,
    placeholder: Text<'a>,
    rows: Option<Amount>,
    readonly: bool,
    common: Common<'a>,
}

impl<'a> Textarea<'a> {
    /// A textarea named `name`.
    pub fn new(name: impl Into<Text<'a>>) -> Self {
        Self {
            value: Text::default(),
            placeholder: Text::default(),
            rows: None,
            readonly: false,
            common: Common {
                name: name.into(),
                ..Common::default()
            },
        }
    }

    /// What it holds as the page opens.
    pub fn value(mut self, value: impl Into<Text<'a>>) -> Self {
        self.value = value.into();
        self
    }

    /// An example of what goes in it, shown while it is empty.
    pub fn placeholder(mut self, placeholder: impl Into<Text<'a>>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// How many lines tall it is.
    pub fn rows(mut self, rows: impl Into<Amount>) -> Self {
        self.rows = Some(rows.into());
        self
    }

    /// Shown but not edited.
    pub fn readonly(mut self) -> Self {
        self.readonly = true;
        self
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, note: Option<&str>, invalid: bool) -> fmt::Result {
        f.write_str("<textarea")?;
        self.common.class(f)?;
        if let Some(placeholder) = self.placeholder.present() {
            attribute(f, "placeholder", placeholder)?;
        }
        if let Some(rows) = self.rows {
            attribute(f, "rows", &rows.to_string())?;
        }
        if self.readonly {
            f.write_str(" readonly")?;
        }
        self.common.write(f, note, invalid)?;
        f.write_str(">")?;
        escape(f, self.value.as_str())?;
        f.write_str("</textarea>")
    }
}

control!(Textarea);

/// A field's control: one of the kit's, or markup of the app's own.
#[derive(Clone, Debug)]
pub enum Control<'a> {
    /// An `<input>`.
    Input(Input<'a>),
    /// A `<select>`.
    Select(Select<'a>),
    /// A `<textarea>`.
    Textarea(Textarea<'a>),
    /// A control the kit lacks, as the app wrote it.
    Custom(Markup),
}

impl Control<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Input(input) => input.common.name.as_str(),
            Self::Select(select) => select.common.name.as_str(),
            Self::Textarea(textarea) => textarea.common.name.as_str(),
            Self::Custom(_) => "",
        }
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, note: Option<&str>, invalid: bool) -> fmt::Result {
        match self {
            Self::Input(input) => input.write(f, note, invalid),
            Self::Select(select) => select.write(f, note, invalid),
            Self::Textarea(textarea) => textarea.write(f, note, invalid),
            Self::Custom(markup) => f.write_str(markup.as_str()),
        }
    }
}

impl From<Markup> for Control<'_> {
    fn from(markup: Markup) -> Self {
        Self::Custom(markup)
    }
}

/// A labelled control with a note under it: `kit::Field::email("email", "email").required().hint("For sign-in only.")`.
///
/// The label leads and the control fills the width. The note is the [`error`](Self::error) when there is one, else
/// the [`warning`](Self::warning), else the [`hint`](Self::hint), and the control is described by it.
#[must_use = "a field draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "field.html")]
pub struct Field<'a> {
    label: Text<'a>,
    control: Control<'a>,
    hint: Content<'a>,
    error: Text<'a>,
    warning: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Field<'a> {
    /// `control`, labelled `label`.
    pub fn new(label: impl Into<Text<'a>>, control: impl Into<Control<'a>>) -> Self {
        Self {
            label: label.into(),
            control: control.into(),
            hint: Content::default(),
            error: Text::default(),
            warning: Text::default(),
            attrs: Attrs::default(),
        }
    }

    /// A field whose control is the markup of a `{% filter kit::wrap(field) %}` block: one the kit lacks.
    pub fn custom(label: impl Into<Text<'a>>) -> Self {
        Self::new(label, Markup::default())
    }

    /// A line of text named `name`.
    pub fn text(name: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        Self::new(label, Input::text(name))
    }

    /// A secret named `name`.
    pub fn password(name: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        Self::new(label, Input::password(name))
    }

    /// An email address named `name`.
    pub fn email(name: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        Self::new(label, Input::email(name))
    }

    /// A URL named `name`.
    pub fn url(name: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        Self::new(label, Input::url(name))
    }

    /// A number named `name`.
    pub fn number(name: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        Self::new(label, Input::number(name))
    }

    /// A choice among `choices`, named `name`.
    pub fn select<C: Into<Choice<'a>>>(
        name: impl Into<Text<'a>>,
        label: impl Into<Text<'a>>,
        choices: impl IntoIterator<Item = C>,
    ) -> Self {
        Self::new(label, Select::new(name).options(choices))
    }

    /// Lines of text named `name`.
    pub fn textarea(name: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        Self::new(label, Textarea::new(name))
    }

    /// A note under the control saying what goes in it. It keeps its case, since it may quote the outside world.
    pub fn hint(mut self, hint: impl Into<Content<'a>>) -> Self {
        self.hint = hint.into();
        self
    }

    /// What is wrong with what was sent, in place of the hint, with the exact fix. Empty is none, so an
    /// `Option<String>` passes as it is.
    pub fn error(mut self, error: impl Into<Text<'a>>) -> Self {
        self.error = error.into();
        self
    }

    /// What to watch for, in place of the hint. Empty is none.
    pub fn warning(mut self, warning: impl Into<Text<'a>>) -> Self {
        self.warning = warning.into();
        self
    }

    /// What the control holds as the page opens: an input's or textarea's text, a select's chosen option.
    pub fn value(mut self, value: impl Into<Text<'a>>) -> Self {
        self.control = match self.control {
            Control::Input(input) => Control::Input(input.value(value)),
            Control::Select(select) => Control::Select(select.selected(value)),
            Control::Textarea(area) => Control::Textarea(area.value(value)),
            other @ Control::Custom(_) => mismatched("value", other),
        };
        self
    }

    /// An example of what goes in it, shown while it is empty.
    pub fn placeholder(mut self, placeholder: impl Into<Text<'a>>) -> Self {
        self.control = match self.control {
            Control::Input(input) => Control::Input(input.placeholder(placeholder)),
            Control::Textarea(area) => Control::Textarea(area.placeholder(placeholder)),
            other => mismatched("placeholder", other),
        };
        self
    }

    /// What a browser may fill it with: `"username"`, `"current-password"`, `"one-time-code"`.
    pub fn autocomplete(self, token: &'static str) -> Self {
        self.input("autocomplete", |input| input.autocomplete(token))
    }

    /// Shown but not edited.
    pub fn readonly(mut self) -> Self {
        self.control = match self.control {
            Control::Input(input) => Control::Input(input.readonly()),
            Control::Textarea(area) => Control::Textarea(area.readonly()),
            other => mismatched("readonly", other),
        };
        self
    }

    /// Must be filled before its form sends.
    pub fn required(self) -> Self {
        self.each(
            "required",
            Input::required,
            Select::required,
            Textarea::required,
        )
    }

    /// Unavailable, and drawn so.
    pub fn disabled(self, on: impl Into<Flag>) -> Self {
        let on = on.into();
        self.each(
            "disabled",
            |input| input.disabled(on),
            |select| select.disabled(on),
            |area| area.disabled(on),
        )
    }

    /// Focused when its page or dialog opens.
    pub fn autofocus(self) -> Self {
        self.each(
            "autofocus",
            Input::autofocus,
            Select::autofocus,
            Textarea::autofocus,
        )
    }

    /// Two-way bound to the Datastar signal `signal`.
    ///
    /// # Panics
    ///
    /// When `signal` is not letters, digits, `_` and `.`.
    pub fn bind(self, signal: &'static str) -> Self {
        self.each(
            "bind",
            |input| input.bind(signal),
            |select| select.bind(signal),
            |area| area.bind(signal),
        )
    }

    /// The least a number takes.
    pub fn min(self, min: impl Into<Amount>) -> Self {
        let min = min.into();
        self.input("min", |input| input.min(min))
    }

    /// The most a number takes.
    pub fn max(self, max: impl Into<Amount>) -> Self {
        let max = max.into();
        self.input("max", |input| input.max(max))
    }

    /// The steps a number moves in.
    pub fn step(self, step: impl Into<Amount>) -> Self {
        let step = step.into();
        self.input("step", |input| input.step(step))
    }

    /// How many lines tall a textarea is.
    pub fn rows(mut self, rows: impl Into<Amount>) -> Self {
        let rows = rows.into();
        self.control = match self.control {
            Control::Textarea(area) => Control::Textarea(area.rows(rows)),
            other => mismatched("rows", other),
        };
        self
    }

    /// The option chosen as the page opens.
    pub fn selected(mut self, value: impl Into<Text<'a>>) -> Self {
        self.control = match self.control {
            Control::Select(select) => Control::Select(select.selected(value)),
            other => mismatched("selected", other),
        };
        self
    }

    /// A prop applied to whichever control the field holds.
    fn each(
        mut self,
        prop: &'static str,
        input: impl FnOnce(Input<'a>) -> Input<'a>,
        select: impl FnOnce(Select<'a>) -> Select<'a>,
        textarea: impl FnOnce(Textarea<'a>) -> Textarea<'a>,
    ) -> Self {
        self.control = match self.control {
            Control::Input(control) => Control::Input(input(control)),
            Control::Select(control) => Control::Select(select(control)),
            Control::Textarea(control) => Control::Textarea(textarea(control)),
            other @ Control::Custom(_) => mismatched(prop, other),
        };
        self
    }

    /// A prop only an input takes.
    fn input(mut self, prop: &'static str, input: impl FnOnce(Input<'a>) -> Input<'a>) -> Self {
        self.control = match self.control {
            Control::Input(control) => Control::Input(input(control)),
            other => mismatched(prop, other),
        };
        self
    }

    /// The note: the error, the warning or the hint, in that order.
    fn note(&self) -> Option<(Option<&'static str>, Content<'_>)> {
        if let Some(error) = self.error.present() {
            Some((Some("bad"), Content::from(error)))
        } else if let Some(warning) = self.warning.present() {
            Some((Some("warn"), Content::from(warning)))
        } else if self.hint.is_empty() {
            None
        } else {
            Some((None, self.hint.clone()))
        }
    }

    /// The note's id: the field's, or its control's name, and `-note`.
    fn note_id(&self) -> String {
        let base = if self.attrs.id().is_empty() {
            self.control.name()
        } else {
            self.attrs.id()
        };
        format!("{base}-note")
    }

    /// The control, described by the note when there is one.
    fn control(&self) -> FieldControl<'_, 'a> {
        FieldControl(self)
    }
}

/// A prop given to a field whose control does not take it: loud in a debug build, since it would otherwise vanish.
#[expect(
    clippy::manual_assert,
    reason = "assert! on cfg! is a constant assertion, which clippy refuses too"
)]
fn mismatched<'a>(prop: &'static str, control: Control<'a>) -> Control<'a> {
    if cfg!(debug_assertions) {
        panic!("this field's control takes no {prop}");
    }
    control
}

/// A field's control as the field draws it: described by its note, and invalid with an error.
struct FieldControl<'f, 'a>(&'f Field<'a>);

impl fmt::Display for FieldControl<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field = self.0;
        let note = field.note().map(|_| field.note_id());
        field
            .control
            .write(f, note.as_deref(), field.error.present().is_some())
    }
}

impl askama::filters::HtmlSafe for FieldControl<'_, '_> {}

impl Wrap for Field<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.control = Control::Custom(content);
        self
    }
}

component!(Field);

/// A control joined to the button it feeds, reading as one: `kit::Combo::new(kit::Input::search("q").label("search"),
/// kit::Button::primary("grab"))`.
#[must_use = "a combo draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "combo.html")]
pub struct Combo<'a> {
    control: Control<'a>,
    button: Button<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Combo<'a> {
    /// `control` and `button`, sharing a border. Name the control with its own `label`, since no field names it.
    pub fn new(control: impl Into<Control<'a>>, button: Button<'a>) -> Self {
        Self {
            control: control.into(),
            button,
            attrs: Attrs::default(),
        }
    }

    fn control(&self) -> ComboControl<'_, 'a> {
        ComboControl(&self.control)
    }
}

struct ComboControl<'c, 'a>(&'c Control<'a>);

impl fmt::Display for ComboControl<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write(f, None, false)
    }
}

impl askama::filters::HtmlSafe for ComboControl<'_, '_> {}

component!(Combo);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hint_is_named_by_its_field() {
        let field = Field::password("token", "api token")
            .hint("Stored sealed.")
            .to_string();
        assert!(field.contains("aria-describedby=\"token-note\""), "{field}");
        assert!(
            field.contains("<small id=\"token-note\">Stored sealed.</small>"),
            "{field}"
        );
        assert!(!field.contains("aria-invalid"), "{field}");
    }

    #[test]
    fn an_error_replaces_the_hint_and_marks_the_control_invalid() {
        let error: Option<String> = Some("Not a URL: it must start with https://.".into());
        let field = Field::url("server", "server url")
            .hint("Your server.")
            .error(&error)
            .to_string();
        assert!(field.contains("class=\"field bad\""), "{field}");
        assert!(field.contains("aria-invalid=\"true\""), "{field}");
        assert!(field.contains("must start with https://."), "{field}");
        assert!(!field.contains("Your server."), "{field}");
        let none: Option<String> = None;
        let fine = Field::url("server", "server url").error(&none).to_string();
        assert!(!fine.contains("bad"), "no error is no error: {fine}");
    }

    #[test]
    fn two_fields_of_one_name_are_told_apart_by_id() {
        let first = Field::text("name", "name")
            .hint("a")
            .id("rename-name")
            .to_string();
        assert!(first.contains("id=\"rename-name-note\""), "{first}");
        assert!(
            first.contains("aria-describedby=\"rename-name-note\""),
            "{first}"
        );
    }

    #[test]
    fn a_select_marks_its_chosen_option() {
        let regions = vec![
            ("eu".to_string(), "eu-west".to_string()),
            ("us".to_string(), "us-east".to_string()),
        ];
        let select = Select::new("region")
            .options(&regions)
            .selected("us")
            .to_string();
        assert!(
            select.contains("<option value=\"us\" selected>us-east</option>"),
            "{select}"
        );
        assert!(
            select.contains("<option value=\"eu\">eu-west</option>"),
            "{select}"
        );
    }

    #[test]
    #[expect(
        clippy::needless_borrows_for_generic_args,
        reason = "the test hands over a reference as a template does"
    )]
    fn a_number_carries_its_bounds() {
        let most = 10_u32;
        let input = Input::number("slots").min(0).max(&most).step(1).to_string();
        assert!(input.contains("min=\"0\" max=\"10\" step=\"1\""), "{input}");
    }
}

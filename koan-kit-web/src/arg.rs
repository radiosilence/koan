//! What a component's props take: text, figures, switches and markup, from whatever a view holds.
//!
//! askama passes a template's variables by reference and its literals and calls by value, so the same prop is handed
//! `"connected"`, a `String` a call returned, `&String` from a field and `&&String` from a loop. Each type here converts
//! from all of them, so a template never writes `*count` or `&name` to reach a component, and a handler passes what it
//! has.

use std::borrow::Cow;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use askama::filters::{Escaper, HtmlSafe};
use askama_datastar::Html;

/// Text a component shows or writes into an attribute: escaped wherever it lands.
///
/// Built from anything a view holds as text, by value or by reference: `&str`, `String`, `Cow<str>`, `Arc<str>`, and an
/// `Option` of any of them, where `None` is no text. A prop that may be left out treats empty text as absent, so an
/// `Option<String>` field passes straight through: `.error(error)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text<'a>(Cow<'a, str>);

impl Text<'_> {
    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether there is no text, which a prop that may be left out reads as absent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text, or `None` when there is none.
    pub(crate) fn present(&self) -> Option<&str> {
        (!self.0.is_empty()).then_some(&*self.0)
    }

    /// The same text, owned, so it can outlive what it was borrowed from.
    #[must_use]
    pub fn into_owned(self) -> Text<'static> {
        Text(Cow::Owned(self.0.into_owned()))
    }
}

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Text a [`Text`] can borrow: the referents of everything a view passes by reference.
///
/// Implemented for the string types and for `Option`s and references of them, so `&&String` from a template loop
/// borrows as readily as `&str`. An app implements it for its own type that reads as text, such as a name.
pub trait AsText {
    /// The text, borrowed.
    fn as_text(&self) -> &str;
}

impl AsText for str {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for String {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for Cow<'_, str> {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for Box<str> {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for Rc<str> {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for Arc<str> {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for Text<'_> {
    fn as_text(&self) -> &str {
        &self.0
    }
}

/// `None` is no text.
impl<T: AsText> AsText for Option<T> {
    fn as_text(&self) -> &str {
        self.as_ref().map_or("", AsText::as_text)
    }
}

impl<T: AsText + ?Sized> AsText for &T {
    fn as_text(&self) -> &str {
        (**self).as_text()
    }
}

impl<'a, T: AsText + ?Sized> From<&'a T> for Text<'a> {
    fn from(text: &'a T) -> Self {
        Self(Cow::Borrowed(text.as_text()))
    }
}

impl From<String> for Text<'_> {
    fn from(text: String) -> Self {
        Self(Cow::Owned(text))
    }
}

impl<'a> From<Cow<'a, str>> for Text<'a> {
    fn from(text: Cow<'a, str>) -> Self {
        Self(text)
    }
}

macro_rules! numbers_as_text {
    ($($number:ty),*) => {$(
        impl From<$number> for Text<'_> {
            fn from(number: $number) -> Self {
                Self(Cow::Owned(number.to_string()))
            }
        }

        impl From<&$number> for Text<'_> {
            fn from(number: &$number) -> Self {
                Self(Cow::Owned(number.to_string()))
            }
        }

        impl From<&&$number> for Text<'_> {
            fn from(number: &&$number) -> Self {
                Self(Cow::Owned(number.to_string()))
            }
        }
    )*};
}

// A count, an id or a page number, as a field's value or a data attribute: `.data("page", page)`.
numbers_as_text!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);

/// `None` is no text.
impl<'a, T: Into<Text<'a>>> From<Option<T>> for Text<'a> {
    fn from(text: Option<T>) -> Self {
        text.map_or_else(Text::default, Into::into)
    }
}

/// A figure: a meter's value, a field's bounds, an icon's size.
///
/// Built from any of Rust's numbers, by value or by reference, so a `u64` field and a float literal both pass.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Amount(f64);

impl Amount {
    /// The figure.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

/// An f32 through its own shortest decimal, so `0.1_f32` is 0.1 rather than its binary expansion widened.
impl From<f32> for Amount {
    fn from(number: f32) -> Self {
        Self(number.to_string().parse().unwrap_or(0.0))
    }
}

/// The shortest form that reads back as the same figure: `40`, `4.5`. A figure that is not one (NaN, infinity) is 0,
/// since no attribute that takes a number accepts either.
impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_finite() {
            write!(f, "{}", self.0)
        } else {
            f.write_str("0")
        }
    }
}

macro_rules! amounts {
    (exact: $($number:ty),*; rounded: $($large:ty),*) => {
        $(
            impl From<$number> for Amount {
                fn from(number: $number) -> Self {
                    Self(f64::from(number))
                }
            }
        )*
        $(
            impl From<$large> for Amount {
                #[expect(clippy::cast_precision_loss, reason = "a figure on a page, where 2^53 is past anything shown")]
                fn from(number: $large) -> Self {
                    Self(number as f64)
                }
            }
        )*
        amounts!(@refs $($number),*, $($large),*, f32);
    };
    (@refs $($number:ty),*) => {$(
        impl From<&$number> for Amount {
            fn from(number: &$number) -> Self {
                Self::from(*number)
            }
        }

        impl From<&&$number> for Amount {
            fn from(number: &&$number) -> Self {
                Self::from(**number)
            }
        }
    )*};
}

amounts!(exact: u8, u16, u32, i8, i16, i32, f64; rounded: u64, u128, usize, i64, i128, isize);

/// A switch: checked, disabled, open, busy.
///
/// Built from a `bool` by value or by reference, so a `bool` field passes as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flag(bool);

impl Flag {
    /// Whether it is on.
    #[must_use]
    pub fn get(self) -> bool {
        self.0
    }
}

impl From<bool> for Flag {
    fn from(on: bool) -> Self {
        Self(on)
    }
}

impl From<&bool> for Flag {
    fn from(on: &bool) -> Self {
        Self(*on)
    }
}

impl From<&&bool> for Flag {
    fn from(on: &&bool) -> Self {
        Self(**on)
    }
}

/// Markup a template made: rendered as it is.
///
/// Built from any of the kit's components, from a rendered template ([`Markup::render`]), from the content of a
/// `{% filter kit::wrap(…) %}` block, and from several of these together (`collect`). Text from anyone else is a
/// [`Text`], which is escaped. Nothing converts a string into markup, an `Html` included, since `Html::trusted` makes
/// one of any string: text cannot be passed where markup goes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Markup(String);

impl Markup {
    /// Markup the kit itself wrote.
    pub(crate) fn kit(markup: String) -> Self {
        Self(markup)
    }

    /// What `template` renders: an app's own view, escaped as askama escapes it, for a slot that takes markup.
    ///
    /// # Errors
    ///
    /// The template's own, which askama reports with the value or filter that failed.
    pub fn render(template: &impl askama::Template) -> askama::Result<Self> {
        template.render().map(Self)
    }

    /// The markup.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether there is none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for Markup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl HtmlSafe for Markup {}

/// Pieces of markup, one after another: a row of buttons, a list of rows.
impl FromIterator<Markup> for Markup {
    fn from_iter<I: IntoIterator<Item = Markup>>(pieces: I) -> Self {
        Self(pieces.into_iter().map(|piece| piece.0).collect())
    }
}

impl From<&Markup> for Markup {
    fn from(markup: &Markup) -> Self {
        markup.clone()
    }
}

/// What a template's loop over markup hands over.
impl From<&&Markup> for Markup {
    fn from(markup: &&Markup) -> Self {
        (*markup).clone()
    }
}

impl From<Markup> for Html {
    fn from(markup: Markup) -> Self {
        Html::trusted(markup.0)
    }
}

/// What a region holds: words, or markup.
///
/// A notice, a pair's value, a sheet's body and a hint each take either, from anything that converts into a [`Text`]
/// or a [`Markup`]: `"Saved."` is escaped, a component or a template the handler rendered is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content<'a> {
    /// Words, escaped where they land.
    Text(Text<'a>),
    /// Markup, as it is.
    Markup(Markup),
}

impl Default for Content<'_> {
    fn default() -> Self {
        Self::Text(Text::default())
    }
}

impl Content<'_> {
    /// Whether it is markup, which may hold blocks, rather than words.
    #[must_use]
    pub fn is_markup(&self) -> bool {
        matches!(self, Self::Markup(_))
    }

    /// Whether there is nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.is_empty(),
            Self::Markup(markup) => markup.is_empty(),
        }
    }
}

/// Words escaped, markup as it is.
impl fmt::Display for Content<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => askama::filters::Html.write_escaped_str(f, text.as_str()),
            Self::Markup(markup) => f.write_str(markup.as_str()),
        }
    }
}

impl HtmlSafe for Content<'_> {}

impl<'a, T: AsText + ?Sized> From<&'a T> for Content<'a> {
    fn from(text: &'a T) -> Self {
        Self::Text(text.into())
    }
}

impl From<String> for Content<'_> {
    fn from(text: String) -> Self {
        Self::Text(text.into())
    }
}

impl<'a> From<Cow<'a, str>> for Content<'a> {
    fn from(text: Cow<'a, str>) -> Self {
        Self::Text(text.into())
    }
}

impl<'a> From<Text<'a>> for Content<'a> {
    fn from(text: Text<'a>) -> Self {
        Self::Text(text)
    }
}

/// `None` is nothing to show.
impl<'a, T: Into<Text<'a>>> From<Option<T>> for Content<'a> {
    fn from(text: Option<T>) -> Self {
        Self::Text(text.into())
    }
}

impl From<Markup> for Content<'_> {
    fn from(markup: Markup) -> Self {
        Self::Markup(markup)
    }
}

impl From<&Markup> for Content<'_> {
    fn from(markup: &Markup) -> Self {
        Self::Markup(markup.clone())
    }
}

impl From<&&Markup> for Content<'_> {
    fn from(markup: &&Markup) -> Self {
        Self::Markup((*markup).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text<'a>(text: impl Into<Text<'a>>) -> String {
        text.into().to_string()
    }

    #[test]
    #[expect(
        clippy::needless_borrows_for_generic_args,
        reason = "the test hands over references as a template does"
    )]
    fn text_comes_from_whatever_a_template_hands_over() {
        let owned = String::from("owned");
        let borrowed: &str = "borrowed";
        let missing: Option<String> = None;
        let present = Some(String::from("present"));
        assert_eq!(text("literal"), "literal");
        assert_eq!(text(owned.clone()), "owned");
        assert_eq!(text(&owned), "owned");
        assert_eq!(
            text(&&owned),
            "owned",
            "a template loop hands over `&&String`"
        );
        assert_eq!(
            text(&borrowed),
            "borrowed",
            "a field holding `&str` arrives as `&&str`"
        );
        assert_eq!(text(&missing), "", "None is no text");
        assert_eq!(text(&present), "present");
        assert_eq!(
            text(Some("call")),
            "call",
            "a call returning an Option passes by value"
        );
        assert_eq!(text(Cow::Borrowed("cow")), "cow");
        assert_eq!(text(&Arc::<str>::from("arc")), "arc");
    }

    #[test]
    fn a_figure_reads_back_as_the_same_number() {
        assert_eq!(Amount::from(40_u64).to_string(), "40");
        assert_eq!(Amount::from(&4.5_f64).to_string(), "4.5");
        assert_eq!(
            Amount::from(&&7_i32).to_string(),
            "7",
            "a template loop hands over `&&i32`"
        );
        assert_eq!(
            Amount::from(f64::NAN).to_string(),
            "0",
            "no attribute accepts NaN"
        );
    }

    #[derive(askama::Template)]
    #[template(ext = "html", source = "<b>{{ name }}</b>")]
    struct Bold<'a> {
        name: &'a str,
    }

    #[derive(askama::Template)]
    #[template(
        ext = "html",
        source = "{% for row in rows %}{{ crate::Notice::new(row) }}{{ crate::Pairs::new().wrap(row.into()) }}{% endfor %}"
    )]
    struct Rows {
        rows: Vec<Markup>,
    }

    #[test]
    fn a_loop_over_markup_hands_each_to_a_slot() {
        use crate::Wrap as _;
        let rows = Rows {
            rows: vec![Markup::kit("<b>a</b>".into())],
        };
        let html = askama::Template::render(&rows).expect("renders");
        assert!(html.contains("<b>a</b>"), "{html}");
        assert_eq!(Markup::from(&&rows.rows[0]), rows.rows[0]);
    }

    #[test]
    fn markup_is_what_a_template_rendered() {
        let markup = Markup::render(&Bold { name: "<i>" }).expect("renders");
        assert_eq!(markup.as_str(), "<b>&#60;i&#62;</b>");
        let both: Markup = [markup.clone(), markup].into_iter().collect();
        assert_eq!(both.as_str().matches("<b>").count(), 2);
    }

    #[test]
    fn content_escapes_words_and_keeps_markup() {
        let words = Content::from("<b>words</b>");
        let markup = Content::from(Markup::kit("<b>markup</b>".into()));
        assert_eq!(words.to_string(), "&#60;b&#62;words&#60;/b&#62;");
        assert_eq!(markup.to_string(), "<b>markup</b>");
    }
}

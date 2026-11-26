//! A box to tick: [`Check`], the box leading its words, inline; and [`Toggle`], a settings row whose box sits at the
//! trailing edge, where a switch would.

use askama::Template;

use crate::arg::{Content, Flag, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::field::signal_name;
use crate::icon::Glyph;
use crate::slot::Wrap;

/// The props a box to tick takes, whichever way round it is drawn.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a prop of its own, as HTML's boolean attributes are"
)]
struct Tick<'a> {
    radio: bool,
    name: Text<'a>,
    value: Option<Text<'a>>,
    label: Content<'a>,
    checked: bool,
    disabled: bool,
    required: bool,
    bind: Option<&'static str>,
}

macro_rules! tick {
    ($tick:ident) => {
        impl<'a> $tick<'a> {
            /// A checkbox named `name`, saying `label`.
            pub fn new(name: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
                Self {
                    tick: Tick {
                        name: name.into(),
                        label: Content::Text(label.into()),
                        ..Tick::default()
                    },
                    ..Self::empty()
                }
            }

            /// One of a set of radios sharing `name`, sending `value` when chosen.
            pub fn radio(
                name: impl Into<Text<'a>>,
                value: impl Into<Text<'a>>,
                label: impl Into<Text<'a>>,
            ) -> Self {
                let mut tick = Self::new(name, label);
                tick.tick.radio = true;
                tick.tick.value = Some(value.into());
                tick
            }

            /// Ticked as the page opens.
            pub fn checked(mut self, on: impl Into<Flag>) -> Self {
                self.tick.checked = on.into().get();
                self
            }

            /// Unavailable, and drawn so.
            pub fn disabled(mut self, on: impl Into<Flag>) -> Self {
                self.tick.disabled = on.into().get();
                self
            }

            /// Must be ticked before its form sends.
            pub fn required(mut self) -> Self {
                self.tick.required = true;
                self
            }

            /// What a ticked checkbox sends, in place of `on`.
            pub fn value(mut self, value: impl Into<Text<'a>>) -> Self {
                self.tick.value = Some(value.into());
                self
            }

            /// Two-way bound to the Datastar signal `signal`: a checkbox's is `true` or `false`, a radio's the value
            /// chosen.
            ///
            /// # Panics
            ///
            /// When `signal` is not letters, digits, `_` and `.`.
            pub fn bind(mut self, signal: &'static str) -> Self {
                self.tick.bind = Some(signal_name(signal));
                self
            }
        }

        impl Wrap for $tick<'_> {
            /// The block's markup in place of the words: words with a field of their own, such as a new album's
            /// title beside the choice to make one.
            fn wrap(mut self, content: Markup) -> Self {
                self.tick.label = content.into();
                self
            }
        }
    };
}

/// What leads a choice's words in a row: a picture, or a glyph.
#[derive(Clone, Debug)]
enum Picture<'a> {
    /// An image at this address; empty draws the empty square.
    Image(Text<'a>),
    /// A glyph of the set.
    Glyph(Glyph),
}

/// A checkbox or radio leading its words, inline: `kit::Check::new("lossless", "lossless only").checked(lossless)`.
///
/// With a [`picture`](Self::picture) or a [`detail`](Self::detail) it is a row of a list to choose from, the box, the
/// picture and two lines of words, as a picker of albums draws each. [`bare`](Self::bare) draws the box alone.
#[must_use = "a check draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "check.html")]
pub struct Check<'a> {
    tick: Tick<'a>,
    bare: bool,
    picture: Option<Picture<'a>>,
    detail: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Check<'a> {
    /// The box alone, named by its label for a screen reader rather than by words beside it: a row's select box. The
    /// common props reach the `<input>`, its outermost element, so `.attr("form", id)` and `.datastar(…)` land on it.
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// A picture between the box and the words, from `src`, such as a record's cover; empty draws the empty square,
    /// so a row without one keeps its place.
    pub fn picture(mut self, src: impl Into<Text<'a>>) -> Self {
        self.picture = Some(Picture::Image(src.into()));
        self
    }

    /// A glyph in the picture's square: a choice to make something new.
    pub fn picture_icon(mut self, glyph: Glyph) -> Self {
        self.picture = Some(Picture::Glyph(glyph));
        self
    }

    /// A second line under the words, muted: a count, a size.
    pub fn detail(mut self, detail: impl Into<Text<'a>>) -> Self {
        self.detail = detail.into();
        self
    }

    fn is_bare(&self) -> bool {
        self.bare
    }

    /// Whether it is drawn as a row of a list.
    fn is_row(&self) -> bool {
        self.picture.is_some() || !self.detail.is_empty()
    }
}

impl Check<'_> {
    fn empty() -> Self {
        Self {
            tick: Tick::default(),
            bare: false,
            picture: None,
            detail: Text::default(),
            attrs: Attrs::default(),
        }
    }
}

tick!(Check);
component!(Check);

/// A settings row: the words leading, the box at the trailing edge. `kit::Toggle::new("notify", "notify when done")`.
#[must_use = "a toggle draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "toggle.html")]
pub struct Toggle<'a> {
    tick: Tick<'a>,
    attrs: Attrs<'a>,
}

impl Toggle<'_> {
    #[expect(
        clippy::unused_self,
        reason = "the box's template asks every tick whether it stands alone"
    )]
    fn is_bare(&self) -> bool {
        false
    }

    fn empty() -> Self {
        Self {
            tick: Tick::default(),
            attrs: Attrs::default(),
        }
    }
}

tick!(Toggle);
component!(Toggle);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(
        clippy::needless_borrows_for_generic_args,
        reason = "the test hands over a reference as a template does"
    )]
    fn a_check_leads_its_words_and_a_toggle_trails_them() {
        let checked = true;
        let check = Check::new("lossless", "lossless only")
            .checked(&checked)
            .to_string();
        assert!(
            check.starts_with(
                "<label class=\"check\"><input type=\"checkbox\" name=\"lossless\" checked>"
            ),
            "{check}"
        );
        let toggle = Toggle::new("notify", "notify when done").to_string();
        assert!(
            toggle.contains("<span>notify when done</span><input type=\"checkbox\""),
            "{toggle}"
        );
    }

    #[test]
    fn a_bare_box_carries_its_props_on_the_input() {
        let html = Check::new("path", "select Untrue")
            .bare()
            .value("/music/untrue")
            .attr("form", "library")
            .class("mt-1")
            .to_string();
        assert!(html.starts_with("<input type=\"checkbox\""), "{html}");
        assert!(html.contains("aria-label=\"select Untrue\""), "{html}");
        assert!(html.contains("form=\"library\""), "{html}");
        assert!(html.contains("class=\"mt-1\""), "{html}");
        assert!(!html.contains("<label"), "{html}");
    }

    #[test]
    fn a_radio_row_carries_a_picture_and_a_detail() {
        let html = Check::radio("to", "a1", "Holiday")
            .picture("/covers/a1.jpg")
            .detail("12 images")
            .to_string();
        assert!(html.contains("class=\"check choice\""), "{html}");
        assert!(
            html.contains("<span class=\"picture\"><img src=\"/covers/a1.jpg\" alt=\"\" loading=\"lazy\"></span>"),
            "{html}"
        );
        assert!(html.contains("<small>12 images</small>"), "{html}");
    }

    #[test]
    fn an_empty_value_is_sent_as_empty_not_on() {
        let radio = Check::radio("quality", "", "any").to_string();
        assert!(radio.contains("value=\"\""), "{radio}");
    }

    #[test]
    fn a_radio_sends_its_value() {
        let radio = Check::radio("quality", "lossless", "lossless").to_string();
        assert!(
            radio.contains("type=\"radio\" name=\"quality\" value=\"lossless\""),
            "{radio}"
        );
    }
}

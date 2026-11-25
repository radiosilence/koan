//! A box to tick: [`Check`], the box leading its words, inline; and [`Toggle`], a settings row whose box sits at the
//! trailing edge, where a switch would.

use askama::Template;

use crate::arg::{Flag, Text};
use crate::attrs::{Attrs, component};
use crate::field::signal_name;

/// The props a box to tick takes, whichever way round it is drawn.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a prop of its own, as HTML's boolean attributes are"
)]
struct Tick<'a> {
    radio: bool,
    name: Text<'a>,
    value: Text<'a>,
    label: Text<'a>,
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
                        label: label.into(),
                        ..Tick::default()
                    },
                    attrs: Attrs::default(),
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
                tick.tick.value = value.into();
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
                self.tick.value = value.into();
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
    };
}

/// A checkbox or radio leading its words, inline: `kit::Check::new("lossless", "lossless only").checked(lossless)`.
#[must_use = "a check draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "check.html")]
pub struct Check<'a> {
    tick: Tick<'a>,
    attrs: Attrs<'a>,
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
    fn a_radio_sends_its_value() {
        let radio = Check::radio("quality", "lossless", "lossless").to_string();
        assert!(
            radio.contains("type=\"radio\" name=\"quality\" value=\"lossless\""),
            "{radio}"
        );
    }
}

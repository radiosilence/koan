//! A button: what it says, how much it weighs, and what it does.
//!
//! One type draws every action, so a request, a link, a command and a submit all look and behave alike. What a
//! button does decides its markup: a request carries its URL in `data-url` and a constant expression that reads it,
//! with the busy signal of that request on the button; a question first opens the page's [`Confirm`](crate::Confirm);
//! a command opens or closes a dialog through `commandfor`, with no script; a link is an `<a>` with a real `href`, so a
//! modified click is the browser's.

use std::fmt;

use crate::action::{Busy, Method};
use crate::arg::{Flag, Text};
use crate::attrs::{Attrs, attribute, component, escape};
use crate::icon::Glyph;
use crate::sheet::{CONFIRM_ID, OPEN_CONFIRM};

/// How much a button weighs on its page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Weight {
    /// Filled with the accent: the one main action of a page, a card or a form.
    Primary,
    /// Outlined: a button's weight unless the page says otherwise, and every action repeated down a long list.
    #[default]
    Bordered,
    /// The label alone, a step up from the text around it.
    Standard,
    /// A bar's text button: muted until hovered.
    Quiet,
    /// Goes somewhere, or a lesser action inline with text: underlined.
    Link,
}

impl Weight {
    /// The kit's class, or none for the bordered default, which is `<button>` as the kit draws it.
    fn class(self) -> Option<&'static str> {
        match self {
            Self::Primary => Some("primary"),
            Self::Bordered => None,
            Self::Standard => Some("standard"),
            Self::Quiet => Some("quiet"),
            Self::Link => Some("link"),
        }
    }

    /// The size a glyph is drawn at beside the label: 1.2 times the label's type, as the stroke rule assumes.
    fn glyph_px(self) -> f64 {
        match self {
            Self::Primary => 18.0,
            Self::Standard => 16.8,
            Self::Bordered | Self::Quiet | Self::Link => 15.6,
        }
    }
}

/// What a button does when pressed.
#[derive(Clone, Debug, Default)]
enum Act<'a> {
    /// Nothing the kit wires: the app's own `.datastar()` or script, or a placeholder.
    #[default]
    Nothing,
    /// Submits its form, busy while the form's request runs when the form is a [`Form`](crate::Form).
    Submit(Option<Busy>),
    /// Goes to `href`, drawn as an `<a>`.
    Link(Text<'a>),
    /// Sends a request, after a question or a preview when one is set.
    Request {
        method: Method,
        url: Text<'a>,
        ask: Option<Ask<'a>>,
    },
    /// Opens or closes the dialog with this id, with no script.
    Command {
        command: &'static str,
        target: Text<'a>,
    },
}

/// What a button asks before its request goes.
#[derive(Clone, Debug)]
struct Ask<'a> {
    question: Text<'a>,
    yes: Text<'a>,
    preview: Text<'a>,
}

/// A button, or a link drawn as one: `kit::Button::primary("connect").post(url)`.
///
/// Its weight is its constructor; [`bad`](Self::bad), [`fill`](Self::fill) and an [`icon`](Self::icon) qualify it,
/// and one action says what it does: [`post`](Self::post), [`get`](Self::get), [`href`](Self::href),
/// [`opens`](Self::opens), [`closes`](Self::closes) or [`submit`](Self::submit). A request can ask first, with
/// [`confirm`](Self::confirm) or [`preview`](Self::preview).
#[must_use = "a button draws nothing until it is rendered"]
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a prop of its own, as HTML's boolean attributes are"
)]
pub struct Button<'a> {
    label: Text<'a>,
    weight: Weight,
    bad: bool,
    fill: bool,
    busy: bool,
    disabled: bool,
    autofocus: bool,
    icon: Option<Glyph>,
    icon_only: bool,
    named: Option<(&'static str, Text<'a>)>,
    act: Act<'a>,
    /// A question given before the request it guards.
    pending: Option<Ask<'a>>,
    attrs: Attrs<'a>,
}

impl<'a> Button<'a> {
    /// A button of `weight` saying `label`.
    pub fn new(weight: Weight, label: impl Into<Text<'a>>) -> Self {
        Self {
            label: label.into(),
            weight,
            bad: false,
            fill: false,
            busy: false,
            disabled: false,
            autofocus: false,
            icon: None,
            icon_only: false,
            named: None,
            act: Act::Nothing,
            pending: None,
            attrs: Attrs::default(),
        }
    }

    /// The one main action of a page, a card or a form, filled with the accent.
    pub fn primary(label: impl Into<Text<'a>>) -> Self {
        Self::new(Weight::Primary, label)
    }

    /// Outlined: the default weight, and every action repeated down a long list.
    pub fn bordered(label: impl Into<Text<'a>>) -> Self {
        Self::new(Weight::Bordered, label)
    }

    /// The label alone, a step up from the text around it.
    pub fn standard(label: impl Into<Text<'a>>) -> Self {
        Self::new(Weight::Standard, label)
    }

    /// A bar's text button, muted until hovered.
    pub fn quiet(label: impl Into<Text<'a>>) -> Self {
        Self::new(Weight::Quiet, label)
    }

    /// Underlined, for an action inline with text.
    pub fn link(label: impl Into<Text<'a>>) -> Self {
        Self::new(Weight::Link, label)
    }

    /// Another weight, for a button whose weight the view decides.
    pub fn weight(mut self, weight: Weight) -> Self {
        self.weight = weight;
        self
    }

    /// Destructive: drawn in the error tone, on any weight.
    pub fn bad(mut self) -> Self {
        self.bad = true;
        self
    }

    /// Filled with its tone rather than outlined: a quiet fill on the default weight, the error colour on a
    /// [`bad`](Self::bad) one.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// A glyph before the label, at the label's size.
    pub fn icon(mut self, glyph: Glyph) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// The glyph alone, on a 40-pixel target, with the label as its accessible name.
    pub fn icon_only(mut self, glyph: Glyph) -> Self {
        self.icon = Some(glyph);
        self.icon_only = true;
        self
    }

    /// Unavailable, and drawn so.
    pub fn disabled(mut self, on: impl Into<Flag>) -> Self {
        self.disabled = on.into().get();
        self
    }

    /// Drawn as running, with the spinner after its label, for a request the server knows is still going. A
    /// request the button sends shows this by itself while it runs.
    pub fn busy(mut self, on: impl Into<Flag>) -> Self {
        self.busy = on.into().get();
        self
    }

    /// Focused when its page or dialog opens: the safe choice in a question, such as cancel.
    pub fn autofocus(mut self) -> Self {
        self.autofocus = true;
        self
    }

    /// The `name` and `value` it submits with its form, so one form can tell its buttons apart: a consent page's
    /// allow and deny.
    pub fn named(mut self, name: &'static str, value: impl Into<Text<'a>>) -> Self {
        self.named = Some((name, value.into()));
        self
    }

    /// Posts to `url` when pressed, busy while its request runs. The URL travels in `data-url` and the expression
    /// that sends it is the kit's own, so nothing in the URL reaches script. The route sits behind
    /// `require_datastar_on_post`.
    pub fn post(mut self, url: impl Into<Text<'a>>) -> Self {
        self.act = Act::Request {
            method: Method::Post,
            url: url.into(),
            ask: self.take_ask(),
        };
        self
    }

    /// Reads `url` when pressed, busy while its request runs: for a fragment the page shows on demand. A change is a
    /// [`post`](Self::post).
    pub fn get(mut self, url: impl Into<Text<'a>>) -> Self {
        self.act = Act::Request {
            method: Method::Get,
            url: url.into(),
            ask: None,
        };
        self
    }

    /// Asks `question` before it posts, in the page's [`Confirm`](crate::Confirm), whose yes says `yes`. The answer
    /// is the button's own request: its URL, and its tone, so a [`bad`](Self::bad) button's yes is destructive too.
    /// A page holding one places `kit::Confirm` once.
    pub fn confirm(mut self, question: impl Into<Text<'a>>, yes: impl Into<Text<'a>>) -> Self {
        self.ask(Ask {
            question: question.into(),
            yes: yes.into(),
            preview: Text::default(),
        });
        self
    }

    /// Shows the server's preview of what the post will change before it goes: the page's
    /// [`Confirm`](crate::Confirm) opens asking `question`, reads `preview` into itself, and enables `yes` only once
    /// this button's preview has arrived. The preview's handler answers with
    /// [`Confirm::preview`](crate::Confirm::preview).
    pub fn preview(
        mut self,
        preview: impl Into<Text<'a>>,
        question: impl Into<Text<'a>>,
        yes: impl Into<Text<'a>>,
    ) -> Self {
        self.ask(Ask {
            question: question.into(),
            yes: yes.into(),
            preview: preview.into(),
        });
        self
    }

    /// Goes to `href`: drawn as a link, so it works before any script and a modified click opens a tab.
    pub fn href(mut self, href: impl Into<Text<'a>>) -> Self {
        self.act = Act::Link(href.into());
        self
    }

    /// Opens the dialog `id` as a modal (a [`Sheet`](crate::Sheet)), through `commandfor`, with no script.
    pub fn opens(mut self, id: impl Into<Text<'a>>) -> Self {
        self.act = Act::Command {
            command: "show-modal",
            target: id.into(),
        };
        self
    }

    /// Closes the dialog `id`, through `commandfor`, with no script: a sheet's cancel.
    pub fn closes(mut self, id: impl Into<Text<'a>>) -> Self {
        self.act = Act::Command {
            command: "close",
            target: id.into(),
        };
        self
    }

    /// Submits its form. [`Form::submit`](crate::Form::submit) gives one that is busy while the form's request runs.
    pub fn submit(mut self) -> Self {
        self.act = Act::Submit(None);
        self
    }

    /// A submit busy by `signal`, the form's.
    pub(crate) fn submits_with(mut self, signal: Busy) -> Self {
        self.act = Act::Submit(Some(signal));
        self
    }

    /// Puts a question on the request this button sends, or keeps it for the request it will be given, so the
    /// order of `.post()` and `.confirm()` does not matter.
    fn ask(&mut self, ask: Ask<'a>) {
        match &mut self.act {
            Act::Request {
                method, ask: slot, ..
            } => {
                debug_assert!(
                    *method == Method::Post,
                    "a question guards a change; a GET changes nothing"
                );
                *slot = Some(ask);
            }
            act => {
                debug_assert!(
                    matches!(act, Act::Nothing),
                    "a question guards a request; give the button .post(url) as well"
                );
                self.pending = Some(ask);
            }
        }
    }

    fn take_ask(&mut self) -> Option<Ask<'a>> {
        self.pending.take()
    }

    /// The classes the kit draws it with, the app's after them.
    fn classes(&self, link: bool) -> Vec<&'static str> {
        let mut classes = Vec::with_capacity(6);
        if link {
            classes.push("button");
        }
        if self.icon_only {
            classes.push("icon-button");
        } else if let Some(class) = self.weight.class() {
            classes.push(class);
        }
        if self.bad {
            classes.push("bad");
        }
        if self.fill {
            classes.push("fill");
        }
        if self.busy {
            classes.push("busy");
        }
        classes
    }
}

impl fmt::Display for Button<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let link = matches!(self.act, Act::Link(_));
        if link {
            f.write_str("<a")?;
        } else {
            let kind = if matches!(self.act, Act::Submit(_)) {
                "submit"
            } else {
                "button"
            };
            write!(f, "<button type=\"{kind}\"")?;
        }
        let classes = self.classes(link);
        if !classes.is_empty() || !self.attrs.classes().is_empty() {
            write!(
                f,
                " class=\"{}{}\"",
                classes.join(" "),
                self.attrs.classes()
            )?;
        }
        match &self.act {
            Act::Nothing => {}
            Act::Link(href) => attribute(f, "href", href.as_str())?,
            Act::Submit(signal) => {
                if let Some(busy) = signal {
                    write!(
                        f,
                        " data-class:busy=\"${busy}\" data-attr:disabled=\"${busy}\""
                    )?;
                }
            }
            Act::Command { command, target } => {
                attribute(f, "commandfor", target.as_str())?;
                write!(f, " command=\"{command}\"")?;
            }
            Act::Request {
                method,
                url,
                ask: None,
            } => {
                let busy = Busy::of(*method, url.as_str());
                attribute(f, "data-url", url.as_str())?;
                write!(
                    f,
                    " data-indicator=\"{busy}\" data-class:busy=\"${busy}\" data-attr:disabled=\"${busy}\" \
                     data-on:click=\"{}(el.dataset.url)\"",
                    method.action()
                )?;
            }
            Act::Request {
                url,
                ask: Some(ask),
                ..
            } => {
                attribute(f, "data-url", url.as_str())?;
                attribute(f, "data-ask", ask.question.as_str())?;
                attribute(f, "data-yes", ask.yes.as_str())?;
                if let Some(preview) = ask.preview.present() {
                    attribute(f, "data-preview", preview)?;
                }
                write!(
                    f,
                    " commandfor=\"{CONFIRM_ID}\" command=\"show-modal\" data-on:click=\"{OPEN_CONFIRM}\""
                )?;
            }
        }
        if let Some((name, value)) = &self.named {
            write!(f, " name=\"{name}\"")?;
            attribute(f, "value", value.as_str())?;
        }
        if self.icon_only {
            attribute(f, "aria-label", self.label.as_str())?;
        }
        if self.disabled {
            f.write_str(if link {
                " aria-disabled=\"true\""
            } else {
                " disabled"
            })?;
        }
        if self.autofocus {
            f.write_str(" autofocus")?;
        }
        write!(f, "{}>", self.attrs)?;
        if let Some(glyph) = self.icon {
            let px = if self.icon_only {
                18.0
            } else {
                self.weight.glyph_px()
            };
            write!(f, "{}", glyph.svg(px))?;
        }
        if !self.icon_only {
            escape(f, self.label.as_str())?;
        }
        f.write_str(if link { "</a>" } else { "</button>" })
    }
}

component!(Button);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon;

    #[test]
    fn a_post_carries_its_url_as_data_and_the_expression_is_constant() {
        let button = Button::quiet("remove")
            .bad()
            .post("/albums/1'); alert(1)//")
            .to_string();
        assert!(
            button.contains("data-url=\"/albums/1&#39;); alert(1)//\""),
            "{button}"
        );
        assert!(
            button.contains("data-on:click=\"@post(el.dataset.url)\""),
            "{button}"
        );
        assert!(button.contains("class=\"quiet bad\""), "{button}");
    }

    #[test]
    fn a_request_is_busy_by_its_own_signal() {
        let button = Button::primary("scan").post("/scan").to_string();
        let busy = Busy::of(Method::Post, "/scan");
        assert!(
            button.contains(&format!("data-indicator=\"{busy}\"")),
            "{button}"
        );
        assert!(
            button.contains(&format!("data-class:busy=\"${busy}\"")),
            "{button}"
        );
        assert!(
            button.contains(&format!("data-attr:disabled=\"${busy}\"")),
            "{button}"
        );
    }

    #[test]
    fn a_link_is_an_anchor_with_a_real_href() {
        let link = Button::primary("sign in").href("/login").to_string();
        assert_eq!(
            link,
            "<a class=\"button primary\" href=\"/login\">sign in</a>"
        );
    }

    #[test]
    fn a_question_is_asked_whichever_order_it_is_given_in() {
        let before = Button::bordered("delete")
            .confirm("Delete it?", "delete")
            .post("/x")
            .to_string();
        let after = Button::bordered("delete")
            .post("/x")
            .confirm("Delete it?", "delete")
            .to_string();
        assert_eq!(before, after);
        assert!(
            before.contains("commandfor=\"kit-confirm\" command=\"show-modal\""),
            "{before}"
        );
        assert!(
            before.contains("data-ask=\"Delete it?\" data-yes=\"delete\""),
            "{before}"
        );
        assert!(
            !before.contains("data-indicator"),
            "the sheet's yes sends the request, not this: {before}"
        );
    }

    #[test]
    fn an_icon_alone_is_named_by_its_label() {
        let button = Button::quiet("more").icon_only(icon::MORE).to_string();
        assert!(button.contains("class=\"icon-button\""), "{button}");
        assert!(button.contains("aria-label=\"more\""), "{button}");
        assert!(
            !button.contains(">more<"),
            "the label is not drawn beside the glyph: {button}"
        );
    }

    #[test]
    fn a_command_needs_no_script() {
        let button = Button::quiet("cancel").closes("add").to_string();
        assert_eq!(
            button,
            "<button type=\"button\" class=\"quiet\" commandfor=\"add\" command=\"close\">cancel</button>"
        );
    }
}

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
use crate::attrs::{Attrs, attribute, component};
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
#[derive(Clone, Debug, askama::Template)]
#[template(path = "button.html")]
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
    /// A line under the question, and whether yes is destructive whatever this button's tone.
    detail: Text<'a>,
    yes_bad: bool,
    /// The busy signal's key, when it is not the request.
    busy_key: Text<'a>,
    /// The id of the form whose fields its request sends.
    form: Text<'a>,
    /// A link that loads its page in place.
    in_place: bool,
    /// Drawn at the size of a compact field.
    compact: bool,
    /// The glyph it shows while pressed, and whether it is.
    toggle: Option<(Glyph, bool)>,
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
            detail: Text::default(),
            yes_bad: false,
            busy_key: Text::default(),
            form: Text::default(),
            in_place: false,
            compact: false,
            toggle: None,
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
    ///
    /// # Panics
    ///
    /// When a question was given: a question guards a change, and a GET changes nothing.
    pub fn get(mut self, url: impl Into<Text<'a>>) -> Self {
        self.refuse_pending("reads with .get()");
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

    /// A line under the question in the page's [`Confirm`](crate::Confirm): what the change does, in more words.
    pub fn detail(mut self, detail: impl Into<Text<'a>>) -> Self {
        self.detail = detail.into();
        self
    }

    /// The question's yes is destructive, though this button is not: a quiet "close unpinned" whose yes still reads
    /// as the loss it is.
    pub fn yes_bad(mut self) -> Self {
        self.yes_bad = true;
        self
    }

    /// Joins the group `key` names: while any of its controls' requests runs, every one of them is disabled, and the
    /// one pressed alone shows the spinner, as a card's actions do. A link in the group ignores a press while one runs.
    /// A key per row keeps rows posting one URL apart.
    pub fn busy_key(mut self, key: impl Into<Text<'a>>) -> Self {
        self.busy_key = key.into();
        self
    }

    /// Sends the fields of the form whose id is `form` with its request, as that form's submit: a form with several
    /// actions gives each its own button, `kit::Button::bordered("enrich").post(enrich).sends("library")`, each posting
    /// to its own URL and busy by itself. The button is a submit of that form, inside it or not, so Enter in a field
    /// presses the form's first one, and the form's `required` and other constraints are checked before anything goes.
    /// With [`confirm`](Self::confirm) the question is asked once the form is valid, and its yes sends the fields.
    pub fn sends(mut self, form: impl Into<Text<'a>>) -> Self {
        self.form = form.into();
        self
    }

    /// For a link: a plain click reads the page in place with `@get`, its handler patching what changes, while a
    /// modified click, a middle click or a page without script follows the real `href`. Busy while it loads, and a
    /// second press meanwhile does nothing. The address becomes the `href` in a history entry of its own, and Back to
    /// an entry made this way reloads it, so a reload, Back and a copied address all name the page shown.
    pub fn in_place(mut self) -> Self {
        self.in_place = true;
        self
    }

    /// The size of a compact field, so it lines up beside one in a row or a toolbar: `meta` type and tight padding. A
    /// `compact` class on any container does the same for every control inside it.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// Shows `glyph` in place of its own while pressed, and says whether it is pressed to a screen reader: play and
    /// pause, mute. The page's script or a Datastar binding sets `aria-pressed` as the state changes; this sets it as
    /// the page opens.
    pub fn toggles(mut self, glyph: Glyph, pressed: impl Into<Flag>) -> Self {
        self.toggle = Some((glyph, pressed.into().get()));
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
    ///
    /// # Panics
    ///
    /// When a question was given: a link goes before anything could ask it.
    pub fn href(mut self, href: impl Into<Text<'a>>) -> Self {
        self.refuse_pending("is a link");
        self.act = Act::Link(href.into());
        self
    }

    /// Opens the dialog `id` as a modal (a [`Sheet`](crate::Sheet)), through `commandfor`, with no script.
    ///
    /// # Panics
    ///
    /// When a question was given, which only a post can ask.
    pub fn opens(mut self, id: impl Into<Text<'a>>) -> Self {
        self.refuse_pending("opens a dialog");
        self.act = Act::Command {
            command: "show-modal",
            target: id.into(),
        };
        self
    }

    /// Closes the dialog `id`, through `commandfor`, with no script: a sheet's cancel.
    ///
    /// # Panics
    ///
    /// When a question was given, which only a post can ask.
    pub fn closes(mut self, id: impl Into<Text<'a>>) -> Self {
        self.refuse_pending("closes a dialog");
        self.act = Act::Command {
            command: "close",
            target: id.into(),
        };
        self
    }

    /// Submits its form. [`Form::submit`](crate::Form::submit) gives one that is busy while the form's request runs.
    ///
    /// # Panics
    ///
    /// When a question was given: a submit would go without asking it. A form that asks first gives its button the
    /// form's action and id, `.post(action).sends(id).confirm(question, yes)`, which keeps Enter and `required`.
    pub fn submit(mut self) -> Self {
        self.refuse_pending("submits its form");
        self.act = Act::Submit(None);
        self
    }

    /// A submit busy by `signal`, the form's.
    pub(crate) fn submits_with(mut self, signal: Busy) -> Self {
        self.refuse_pending("submits its form");
        self.act = Act::Submit(Some(signal));
        self
    }

    /// Puts a question on the request this button sends, or keeps it for the request it will be given, so the
    /// order of `.post()` and `.confirm()` does not matter.
    ///
    /// A question that could not be asked is refused in every build, since dropping it would let a destructive press
    /// go without asking.
    fn ask(&mut self, ask: Ask<'a>) {
        match &mut self.act {
            Act::Request {
                method, ask: slot, ..
            } => {
                assert!(
                    *method == Method::Post,
                    "a question guards a change; this button reads with .get(), which changes nothing"
                );
                *slot = Some(ask);
            }
            Act::Nothing => self.pending = Some(ask),
            _ => panic!("a question guards a .post(); this button does something else"),
        }
    }

    /// Refuses a question given before an action that cannot ask it.
    fn refuse_pending(&self, does: &str) {
        assert!(
            self.pending.is_none(),
            "a question guards a .post(); this button {does}, so the question would never be asked"
        );
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
        if self.compact {
            classes.push("compact");
        }
        classes
    }
}

impl Button<'_> {
    /// Its own busy signal, named by what it is: how it goes and where, the form it sends, its group, its id and its
    /// label. Two buttons alike in all of these are told apart by an [`id`](Self::id).
    fn own_busy(&self, method: &str, url: &str) -> Busy {
        Busy::own(&[
            method,
            url,
            self.form.as_str(),
            self.busy_key.as_str(),
            self.attrs.id(),
            self.label.as_str(),
        ])
    }

    /// Busy and disabled while `signal`'s request runs. What the server set (`.busy(true)`, `.disabled(true)`) is
    /// folded into the expressions, since Datastar's class and attr plugins own the attribute once they bind and would
    /// strip a static one as the signal starts false.
    fn busy_attributes(&self, f: &mut fmt::Formatter<'_>, signal: Busy) -> fmt::Result {
        self.busy_by(f, signal, signal)
    }

    /// Spinning while `busy` is set, disabled while `disabled` is.
    fn busy_by(&self, f: &mut fmt::Formatter<'_>, busy: Busy, disabled: Busy) -> fmt::Result {
        let spinning = if self.busy { "true || " } else { "" };
        let off = if self.disabled || self.busy {
            "true || "
        } else {
            ""
        };
        write!(
            f,
            " data-class:busy=\"{spinning}${busy}\" data-attr:disabled=\"{off}${disabled}\""
        )
    }
}

/// A request's options beyond the URL: its form's fields, and cancellation off for a group.
fn request_options(form: bool, group: Busy) -> String {
    let mut options = Vec::with_capacity(3);
    if form {
        options.push("contentType: 'form', selector: '#' + CSS.escape(el.form.id)");
    }
    let more = group.options();
    if !more.is_empty() {
        options.push(more);
    }
    if options.is_empty() {
        String::new()
    } else {
        format!(", {{{}}}", options.join(", "))
    }
}

/// What a plain click on a link that loads in place does; a modified or middle click is the browser's.
pub(crate) const FOLLOW_GUARD: &str = "evt.metaKey || evt.ctrlKey || evt.shiftKey || evt.altKey || evt.button || (evt.preventDefault(), ";

/// What a link that loaded in place does once its request is sent: the address becomes its `href`, in a history entry
/// of its own, and the entry it left is marked too, so that Back, Forward, a reload and a copied address all name the
/// page shown. A step through the history to a marked entry reloads it, since the page holds whatever was loaded in
/// place last, unless the page has a [`Navigation`](crate::Navigation), which loads it in place instead; the listener
/// is the window's, added once, so it outlives the link a patch may remove.
pub(crate) const FOLLOW_HISTORY: &str = "history.state?.kit || history.replaceState({kit: 1}, ''), \
history.pushState({kit: 1}, '', el.href), \
window._kithistory ??= (addEventListener('popstate', e => e.state?.kit &amp;&amp; !document.getElementById('kit-navigation') &amp;&amp; location.reload()), 1)";

/// Writes the attributes that make a link load in place, busy by its own signal and ignored while its group's runs.
pub(crate) fn in_place_attributes(
    f: &mut fmt::Formatter<'_>,
    own: Busy,
    key: &Text<'_>,
) -> fmt::Result {
    let (own, group) = indicators(f, own, key)?;
    write!(
        f,
        " data-class:busy=\"${own}\" data-on:click=\"{FOLLOW_GUARD}${group} || (@get(el.getAttribute('href'){}), {FOLLOW_HISTORY}))\"",
        request_options(false, group)
    )
}

impl<'a> Button<'a> {
    /// Whether it is drawn as a link.
    fn is_link(&self) -> bool {
        matches!(self.act, Act::Link(_))
    }

    /// Whether it sends a form's fields with its request.
    fn sends_form(&self) -> bool {
        !self.form.is_empty() && matches!(self.act, Act::Request { .. })
    }

    /// Its `type`: a submit when it submits or sends a form.
    fn kind(&self) -> &'static str {
        if self.sends_form() || matches!(self.act, Act::Submit(_)) {
            "submit"
        } else {
            "button"
        }
    }

    /// Its `class`: the kit's, then the app's.
    fn class_attr(&self) -> Option<String> {
        let class = format!(
            "{}{}",
            self.classes(self.is_link()).join(" "),
            self.attrs.classes()
        );
        let class = class.trim_start();
        (!class.is_empty()).then(|| class.to_owned())
    }

    /// The size its glyphs are drawn at.
    fn px(&self) -> f64 {
        if self.compact {
            15.6
        } else if self.icon_only {
            18.0
        } else {
            self.weight.glyph_px()
        }
    }

    /// What it does, as attributes.
    fn wiring(&self) -> Wiring<'_, 'a> {
        Wiring(self)
    }
}

/// A button's attributes between its class and the common props: what it does, its name and value, and its state.
struct Wiring<'b, 'a>(&'b Button<'a>);

impl askama::filters::HtmlSafe for Wiring<'_, '_> {}

impl fmt::Display for Wiring<'_, '_> {
    #[expect(
        clippy::too_many_lines,
        reason = "one element's attributes written in order, read top to bottom"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let button = self.0;
        let link = button.is_link();
        let sends = button.sends_form();
        if sends && let Act::Request { method, url, .. } = &button.act {
            // The form's own submit, should one be made before Datastar loads: refused by `require_datastar_on_post`,
            // as a Form's is, or read as a page.
            attribute(f, "form", button.form.as_str())?;
            attribute(f, "formaction", url.as_str())?;
            if *method == Method::Get {
                f.write_str(" formmethod=\"get\"")?;
            }
        }
        match &button.act {
            // A disabled link has no href, so it neither navigates nor takes focus.
            Act::Link(href) if !button.disabled => {
                attribute(f, "href", href.as_str())?;
                if button.in_place {
                    in_place_attributes(
                        f,
                        button.own_busy("link", href.as_str()),
                        &button.busy_key,
                    )?;
                }
            }
            Act::Nothing | Act::Link(_) => {}
            Act::Submit(signal) => {
                if let Some(busy) = signal {
                    button.busy_attributes(f, *busy)?;
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
                attribute(f, "data-url", url.as_str())?;
                let (own, group) = indicators(
                    f,
                    button.own_busy(method.form(), url.as_str()),
                    &button.busy_key,
                )?;
                button.busy_by(f, own, group)?;
                // A form left invalid would start a request Datastar abandons unsettled, keeping the button busy for
                // good, so the form is checked here first, its messages shown, as a Form's submit does.
                let prevent = if sends {
                    "evt.preventDefault(), (el.form.noValidate || el.form.reportValidity()) &amp;&amp; "
                } else {
                    ""
                };
                write!(
                    f,
                    " data-on:click=\"{prevent}{}(el.dataset.url{})\"",
                    method.action(),
                    request_options(sends, group)
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
                if let Some(detail) = button.detail.present() {
                    attribute(f, "data-detail", detail)?;
                }
                if button.yes_bad {
                    f.write_str(" data-yes-bad")?;
                }
                if sends {
                    // A submit cannot also be a command, so it opens the sheet itself, once the form is valid: the
                    // browser's own messages say what is missing, and Enter in a field asks as a press does.
                    write!(
                        f,
                        " data-on:click=\"evt.preventDefault(), (el.form.noValidate || el.form.reportValidity()) &amp;&amp; ({OPEN_CONFIRM}, \
                         document.getElementById('{CONFIRM_ID}').showModal())\""
                    )?;
                } else {
                    write!(
                        f,
                        " commandfor=\"{CONFIRM_ID}\" command=\"show-modal\" data-on:click=\"{OPEN_CONFIRM}\""
                    )?;
                }
            }
        }
        if let Some((name, value)) = &button.named {
            write!(f, " name=\"{name}\"")?;
            attribute(f, "value", value.as_str())?;
        }
        if button.icon_only {
            attribute(f, "aria-label", button.label.as_str())?;
        }
        if let Some((_, pressed)) = button.toggle {
            write!(f, " aria-pressed=\"{pressed}\"")?;
        }
        if button.disabled {
            f.write_str(if link {
                " aria-disabled=\"true\""
            } else {
                " disabled"
            })?;
        }
        if button.autofocus {
            f.write_str(" autofocus")?;
        }
        Ok(())
    }
}

/// Declares the signals a request is busy by, as the indicators of the element that sends it: its own, which spins
/// it, and its group's, which disables every control in the group, or its own again when it has none.
fn indicators(
    f: &mut fmt::Formatter<'_>,
    own: Busy,
    key: &Text<'_>,
) -> Result<(Busy, Busy), fmt::Error> {
    write!(f, " data-indicator=\"{own}\"")?;
    let group = match key.present() {
        Some(key) => {
            let group = Busy::keyed(key);
            write!(f, " data-indicator:{group}")?;
            group
        }
        None => own,
    };
    Ok((own, group))
}

component!(Button);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon;
    use crate::link::Link;

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

    /// The busy signal a rendered control declares.
    fn signal(html: &str) -> &str {
        let (_, rest) = html
            .split_once("data-indicator=\"")
            .expect("a request declares its busy signal");
        rest.split('"').next().expect("the attribute is closed")
    }

    #[test]
    fn a_request_is_busy_by_its_own_signal() {
        let button = Button::primary("scan").post("/scan").to_string();
        let busy = signal(&button);
        assert!(
            button.contains(&format!("data-class:busy=\"${busy}\"")),
            "{button}"
        );
        assert!(
            button.contains(&format!("data-attr:disabled=\"${busy}\"")),
            "{button}"
        );
        let other = Button::bordered("retry").post("/scan").to_string();
        assert_ne!(
            busy,
            signal(&other),
            "a patch adding or removing one would clear the other's busy state"
        );
        let twin = Button::primary("scan")
            .post("/scan")
            .id("scan-2")
            .to_string();
        assert_ne!(busy, signal(&twin), "an id tells two alike apart");
    }

    #[test]
    fn the_same_button_draws_the_same_markup() {
        let draw = || {
            Button::bordered("retry")
                .post("/jobs/1/retry")
                .busy_key("card-1")
                .to_string()
        };
        assert_eq!(
            draw(),
            draw(),
            "a stream that sends a region only when it differs would resend it"
        );
        let link = || Link::new("next", "/p/2").in_place().to_string();
        assert_eq!(link(), link());
    }

    #[test]
    fn a_shared_key_disables_the_group_and_spins_the_one_pressed() {
        let first = Button::bordered("retry")
            .post("/jobs/1/retry")
            .busy_key("card-1")
            .to_string();
        let second = Button::bordered("next")
            .post("/jobs/1/next")
            .busy_key("card-1")
            .to_string();
        let group = Busy::keyed("card-1");
        for button in [&first, &second] {
            assert!(
                button.contains(&format!("data-indicator:{group}")),
                "{button}"
            );
            assert!(
                button.contains(&format!("data-attr:disabled=\"${group}\"")),
                "{button}"
            );
            let own = signal(button);
            assert!(
                button.contains(&format!("data-class:busy=\"${own}\"")),
                "only the one pressed spins: {button}"
            );
        }
        assert_ne!(signal(&first), signal(&second));
        assert!(
            first.contains("@post(el.dataset.url, {requestCancellation: 'disabled'})"),
            "{first}"
        );
    }

    #[test]
    fn a_button_sends_its_forms_fields_to_its_own_url() {
        let button = Button::bordered("enrich")
            .post("/library/enrich")
            .sends("library")
            .to_string();
        assert!(
            button.starts_with("<button type=\"submit\""),
            "Enter in a field presses it: {button}"
        );
        assert!(
            button.contains("form=\"library\" formaction=\"/library/enrich\""),
            "{button}"
        );
        assert!(
            button.contains(
                "data-on:click=\"evt.preventDefault(), (el.form.noValidate || el.form.reportValidity()) &amp;&amp; @post(el.dataset.url, {contentType: 'form', selector: '#' + CSS.escape(el.form.id)})\""
            ),
            "{button}"
        );
    }

    #[test]
    fn a_form_that_asks_first_checks_itself_then_asks() {
        let button = Button::primary("ban")
            .post("/bans")
            .sends("ban")
            .confirm("ban this user?", "ban")
            .to_string();
        assert!(button.contains("type=\"submit\""), "{button}");
        assert!(
            !button.contains("commandfor"),
            "a submit is not a command: {button}"
        );
        assert!(
            button.contains("(el.form.noValidate || el.form.reportValidity()) &amp;&amp; ("),
            "required is kept: {button}"
        );
        assert!(button.contains("showModal()"), "{button}");
    }

    #[test]
    fn a_link_loads_in_place_and_a_toggle_shows_its_state() {
        let link = Button::link("up").href("/browse?up").in_place().to_string();
        assert!(link.contains("href=\"/browse?up\""), "{link}");
        assert!(link.contains("@get(el.getAttribute('href')"), "{link}");
        let play = Button::quiet("play")
            .icon_only(icon::PLAY)
            .toggles(icon::PAUSE, false)
            .to_string();
        assert!(play.contains("aria-pressed=\"false\""), "{play}");
        assert_eq!(play.matches("<svg").count(), 2, "{play}");
    }

    #[test]
    #[should_panic(expected = "would never be asked")]
    fn a_question_before_a_submit_is_refused() {
        let _ = Button::primary("delete account")
            .bad()
            .confirm("delete your account?", "delete")
            .submit();
    }

    #[test]
    #[should_panic(expected = "changes nothing")]
    fn a_question_on_a_read_is_refused() {
        let _ = Button::bordered("export")
            .get("/export")
            .confirm("export?", "export");
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
    fn what_the_server_set_survives_datastar_binding() {
        let button = Button::primary("rescan")
            .post("/scan")
            .busy(true)
            .to_string();
        let busy = signal(&button);
        assert!(
            button.contains(&format!("data-class:busy=\"true || ${busy}\"")),
            "{button}"
        );
        let off = Button::bordered("delete")
            .post("/x")
            .disabled(true)
            .to_string();
        assert!(off.contains("data-attr:disabled=\"true || $"), "{off}");
    }

    #[test]
    fn a_disabled_link_goes_nowhere() {
        let link = Button::primary("download")
            .href("/d")
            .disabled(true)
            .to_string();
        assert!(!link.contains("href"), "{link}");
        assert!(link.contains("aria-disabled=\"true\""), "{link}");
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

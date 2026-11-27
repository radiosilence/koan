//! Sheets: a dialog that rises from the bottom on a phone and is a centred panel elsewhere, and the one sheet a page
//! keeps for asking before a change.
//!
//! A sheet is a `<dialog>` opened by a button's `commandfor` and closed by `command="close"`, a press outside it or
//! Escape, so opening one needs no script (§8). [`Confirm`] is the sheet every [`Button::confirm`](crate::Button::confirm)
//! and [`Button::preview`](crate::Button::preview) on the page opens: the button hands it the question, the action and
//! its tone through signals, the sheet asks, and its yes sends the button's request. One per page, so a list of a
//! thousand rows carries no thousand dialogs.

use askama::Template;
use askama_datastar::{ElementPatchMode, Html, PatchElements, PatchSignals};
use axum::response::sse::Event;

use crate::arg::{Content, Markup, Text};
use crate::attrs::{Attrs, component, render};
use crate::button::Button;
use crate::slot::Wrap;

/// The id of the page's [`Confirm`].
pub(crate) const CONFIRM_ID: &str = "kit-confirm";

/// What a button that asks does when pressed, alongside opening the sheet: it hands the sheet its question, yes, URL,
/// preview, tone and form from its own attributes and class, so no text reaches the expression; marks the sheet ready
/// at once when there is no preview to wait for; keeps the button, to tell it when the answer succeeds; and counts the
/// opening, which empties the last preview and fetches this one. One expression, its parts joined by commas, so a
/// button that sends a form can run it inside its own.
pub(crate) const OPEN_CONFIRM: &str = "$_kitconfirm.ask = el.dataset.ask, $_kitconfirm.yes = el.dataset.yes, \
$_kitconfirm.url = el.dataset.url, $_kitconfirm.preview = el.dataset.preview || '', \
$_kitconfirm.detail = el.dataset.detail || '', $_kitconfirm.form = el.getAttribute('form') || '', \
$_kitconfirm.bad = el.classList.contains('bad') || 'yesBad' in el.dataset, $_kitconfirm.ready = !el.dataset.preview, \
$_kitconfirm.done = false, document.getElementById('kit-confirm')._kitopener = el, \
$_kitconfirm.open = true, $_kitconfirm.n++";

/// The event the button that asked hears once its answer succeeded, that is once the action's handler closed the
/// sheet with [`Confirm::close`]. It bubbles, and `detail.url` is the URL that was posted.
pub const CONFIRMED: &str = "kit-confirmed";

/// A sheet: `kit::Sheet::new("add", "add a download")`, opened by `kit::Button::primary("add").opens("add")`.
///
/// Its body is [`body`](Self::body), or the markup of a `{% filter kit::wrap(sheet) %}` block; its
/// [`actions`](Self::action) sit along its foot.
#[must_use = "a sheet draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "sheet.html")]
pub struct Sheet<'a> {
    title: Text<'a>,
    close: Text<'a>,
    body: Content<'a>,
    actions: Vec<Button<'a>>,
    attrs: Attrs<'a>,
}

impl<'a> Sheet<'a> {
    /// A sheet with the id its opening button names, headed `title`.
    pub fn new(id: impl Into<Text<'a>>, title: impl Into<Text<'a>>) -> Self {
        let mut attrs = Attrs::default();
        attrs.set_id(id.into());
        Self {
            title: title.into(),
            close: Text::default(),
            body: Content::default(),
            actions: Vec::new(),
            attrs,
        }
    }

    /// A close button in its header, saying `label`. Without one it closes from its actions, a press outside it, or
    /// Escape.
    pub fn close(mut self, label: impl Into<Text<'a>>) -> Self {
        self.close = label.into();
        self
    }

    /// What it holds: words, or markup the handler rendered.
    pub fn body(mut self, body: impl Into<Content<'a>>) -> Self {
        self.body = body.into();
        self
    }

    /// An action along its foot, after any already given: `kit::Button::quiet("cancel").closes("add")`.
    pub fn action(mut self, button: Button<'a>) -> Self {
        self.actions.push(button);
        self
    }
}

impl Wrap for Sheet<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.body = content.into();
        self
    }
}

component!(Sheet);

/// The page's one sheet for asking before a change: place `kit::Confirm::new("cancel")` once, in the page's shell, and
/// every [`Button::confirm`](crate::Button::confirm) and [`Button::preview`](crate::Button::preview) on it asks here.
///
/// Opening it empties whatever it last held and disables its yes until this button's question is in place: at once
/// for a question, and for a preview once [`Confirm::preview`] has answered for this opening, since every opening,
/// a question's too, aborts the last one's preview request. Its yes is busy while the request runs, and the sheet stays open until the
/// action's handler closes it with [`Confirm::close`], so a failure is seen where it happened.
#[must_use = "a sheet draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "confirm.html")]
pub struct Confirm<'a> {
    cancel: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Confirm<'a> {
    /// The sheet, whose cancel says `cancel`.
    pub fn new(cancel: impl Into<Text<'a>>) -> Self {
        let mut attrs = Attrs::default();
        attrs.set_id(Text::from(CONFIRM_ID));
        Self {
            cancel: cancel.into(),
            attrs,
        }
    }
}

impl Confirm<'_> {
    /// A preview handler's answer: `content` in the sheet, and its yes enabled. `events(kit::Confirm::preview(plan))`.
    pub fn preview(content: impl Into<Markup>) -> [Event; 2] {
        let content: Markup = content.into();
        [
            Html::from(content)
                .patch()
                .selector("#kit-confirm-preview")
                .mode(ElementPatchMode::Inner)
                .into(),
            PatchSignals::new(r#"{"_kitconfirm":{"ready":true}}"#).into(),
        ]
    }

    /// A handler's answer when the change, or its preview, failed: `message` inside the sheet, which stays open, since
    /// a flash would sit under the dimmed page. Words are escaped; markup a template rendered, such as a reason with a
    /// link, is kept. The next opening clears it.
    pub fn error<'m>(message: impl Into<Content<'m>>) -> PatchElements {
        render(&ConfirmError {
            message: message.into(),
        })
        .patch()
        .mode(ElementPatchMode::Replace)
    }

    /// An action handler's last word once its change is made: the sheet closes, and the button that asked hears
    /// [`CONFIRMED`], so the page's own script can follow the change. Without it the sheet stays open, so a handler that
    /// fails says so in place.
    #[must_use]
    pub fn close() -> PatchSignals {
        PatchSignals::new(r#"{"_kitconfirm":{"open":false,"done":true}}"#)
    }

    /// An action handler's last word when the page it was asked on is gone, such as the album just deleted: the
    /// browser goes to `href`, a path on this site. No [`close`](Self::close) is needed, since the page goes with the
    /// sheet. Any handler can answer with it.
    ///
    /// It is `askama_datastar::go`, which the kit defers to.
    ///
    /// # Panics
    ///
    /// When `href` is not a path on this origin: it must start with `/`, and not with `//` or `/\\`, which a browser
    /// reads as another host, and hold no whitespace or control character, which a browser strips before reading it,
    /// so `/\t/host` would be `//host`.
    pub fn go<'h>(href: impl Into<Text<'h>>) -> PatchElements {
        askama_datastar::go(href.into().as_str())
    }
}

component!(Confirm);

/// A failure said inside the page's [`Confirm`]: words in a paragraph, markup in a block.
#[derive(Template)]
#[template(path = "confirm_error.html")]
struct ConfirmError<'m> {
    message: Content<'m>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sheet_is_named_by_its_title_and_closes_with_no_script() {
        let sheet = Sheet::new("add", "add a download")
            .close("close")
            .body("Paste a link.")
            .to_string();
        assert!(sheet.contains("aria-labelledby=\"add-title\""), "{sheet}");
        assert!(
            sheet.contains("<h2 id=\"add-title\">add a download</h2>"),
            "{sheet}"
        );
        assert!(
            sheet.contains("commandfor=\"add\" command=\"close\""),
            "{sheet}"
        );
        assert!(sheet.contains("closedby=\"any\""), "{sheet}");
    }

    #[test]
    fn confirm_cannot_be_answered_before_its_own_preview_arrives() {
        let sheet = Confirm::new("cancel").to_string();
        assert!(
            sheet.contains("data-attr:disabled=\"!$_kitconfirm.ready || $_kitconfirmbusy\""),
            "yes must wait for the sheet to be ready: {sheet}"
        );
        assert!(
            OPEN_CONFIRM.contains("$_kitconfirm.ready = !el.dataset.preview"),
            "opening for a preview must clear readiness until the preview lands"
        );
        assert!(
            sheet.contains("el._kitpreview?.abort(), el.replaceChildren()"),
            "every opening must abort the last one's preview, or a late answer could land under another question"
        );
        let [_, ready] = Confirm::preview(Markup::kit("<p>two files</p>".into()));
        assert!(
            format!("{ready:?}").contains("ready"),
            "the preview's answer readies the sheet"
        );
    }

    #[test]
    fn an_error_shows_inside_the_sheet_escaped() {
        let patch = format!("{:?}", Confirm::error("refused: <403>"));
        assert!(patch.contains("kit-confirm-error"), "{patch}");
        assert!(patch.contains("&#60;403&#62;"), "{patch}");
    }

    #[test]
    #[should_panic(expected = "/\\t/evil.example")]
    fn go_refuses_a_path_a_browser_would_read_as_another_host() {
        let _ = Confirm::go("/\t/evil.example");
    }

    #[test]
    fn markup_in_an_error_is_a_block() {
        let patch = format!("{:?}", Confirm::error(Markup::kit("<p>a</p>".into())));
        assert!(
            patch.contains("<div id=\\\"kit-confirm-error\\\""),
            "{patch}"
        );
    }

    #[test]
    fn a_running_answer_cannot_be_dismissed() {
        let sheet = Confirm::new("cancel").to_string();
        assert!(
            sheet.contains("data-attr:closedby=\"$_kitconfirmbusy ? 'none' : 'any'\""),
            "{sheet}"
        );
    }

    #[test]
    fn a_confirm_opener_reads_only_its_own_data() {
        assert!(
            !OPEN_CONFIRM.contains('"'),
            "the expression sits in a double-quoted attribute"
        );
        for read in [
            "el.dataset.ask",
            "el.dataset.yes",
            "el.dataset.url",
            "el.dataset.preview",
        ] {
            assert!(OPEN_CONFIRM.contains(read), "{read}");
        }
    }
}

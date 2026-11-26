//! What a page says about itself: a [`Notice`] in place, a [`Flash`] over the page, an [`Empty`] state where a list
//! would be.

use askama::Template;

use crate::arg::{Content, Markup, Text};
use crate::attrs::{Attrs, component};
use crate::button::Button;
use crate::icon::Glyph;
use crate::slot::Wrap;
use crate::status::Tone;

/// A message about the page, in place: a result, or with a tone a warning or an error. `kit::Notice::bad(error)`.
///
/// An error is an alert, read out as it appears; the rest are statuses, read out when they change. Its words are
/// [`Content`], or the markup of a `{% filter kit::wrap(notice) %}` block, and an [`action`](Self::action) follows them
/// on the same line.
#[must_use = "a notice draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "notice.html")]
pub struct Notice<'a> {
    content: Content<'a>,
    tone: Option<Tone>,
    icon: Option<Glyph>,
    action: Option<Button<'a>>,
    attrs: Attrs<'a>,
}

impl<'a> Notice<'a> {
    /// A result, in the accent.
    pub fn new(content: impl Into<Content<'a>>) -> Self {
        Self {
            content: content.into(),
            tone: None,
            icon: None,
            action: None,
            attrs: Attrs::default(),
        }
    }

    /// A warning.
    pub fn warn(content: impl Into<Content<'a>>) -> Self {
        Self::new(content).tone(Tone::Warn)
    }

    /// An error: an alert, read out as it appears.
    pub fn bad(content: impl Into<Content<'a>>) -> Self {
        Self::new(content).tone(Tone::Bad)
    }

    /// The tone, for a notice whose tone the view decides. `Ok` and `Wait` draw as a plain result.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = matches!(tone, Tone::Warn | Tone::Bad).then_some(tone);
        self
    }

    /// A glyph before the words.
    pub fn icon(mut self, glyph: Glyph) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// An action after the words: retry, undo, sign in.
    pub fn action(mut self, button: Button<'a>) -> Self {
        self.action = Some(button);
        self
    }

    fn role(&self) -> &'static str {
        if self.tone == Some(Tone::Bad) {
            "alert"
        } else {
            "status"
        }
    }
}

impl Wrap for Notice<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.content = content.into();
        self
    }
}

component!(Notice);

/// The page's region for messages that float over it, and the messages a handler sends there.
///
/// Place `kit::Flash::region()` once in the page's shell; a handler answers `kit::Flash::ok("Saved.").patch()`, which
/// shows over the page wherever the tap was and fades by itself, or `kit::Flash::bad(error).patch()`, which stays until
/// it is dismissed or replaced. The region is a live region, so each is read out as it arrives.
///
/// The page's own script raises one by dispatching [`Flash::EVENT`] on `window`, its detail the same props:
///
/// ```js
/// dispatchEvent(new CustomEvent("kit-flash", {detail: {message: "Moved 3 images.", href: "/albums/2", link: "open it"}}))
/// ```
///
/// `message` is the words, `bad` an error, `href` and `link` a link after the words, and `stays` an ok flash that waits
/// to be dismissed. The words are set as text, never as markup.
#[must_use = "a flash draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "flash.html")]
pub struct Flash<'a> {
    message: Text<'a>,
    bad: bool,
    stays: bool,
    link: Option<(Text<'a>, Text<'a>)>,
    dismiss: Text<'a>,
    attrs: Attrs<'a>,
}

impl<'a> Flash<'a> {
    /// The id of the region, which every flash replaces.
    pub const ID: &'static str = "flash";

    /// The event on `window` that raises a flash from the page's own script.
    pub const EVENT: &'static str = "kit-flash";

    fn of(message: Text<'a>, bad: bool) -> Self {
        let mut attrs = Attrs::default();
        attrs.set_id(Self::ID.into());
        Self {
            message,
            bad,
            stays: false,
            link: None,
            dismiss: Text::default(),
            attrs,
        }
    }

    /// The empty region, for the page's shell.
    pub fn region() -> Flash<'static> {
        Flash::of(Text::default(), false)
    }

    /// A confirmation: seen, then gone by itself.
    pub fn ok(message: impl Into<Text<'a>>) -> Self {
        Self::of(message.into(), false)
    }

    /// An error, which stays until it is dismissed or replaced.
    pub fn bad(message: impl Into<Text<'a>>) -> Self {
        Self::of(message.into(), true)
    }

    /// Stays until it is dismissed or replaced, as an error does: a confirmation the reader may still be reading when a
    /// brief one would have gone.
    pub fn stays(mut self) -> Self {
        self.stays = true;
        self
    }

    /// A link after the words, to `href`, saying `label`: "open that album". A flash with a link stays, since it would
    /// otherwise fade from under the pointer.
    pub fn link(mut self, href: impl Into<Text<'a>>, label: impl Into<Text<'a>>) -> Self {
        self.link = Some((href.into(), label.into()));
        self
    }

    /// Whether it waits to be dismissed rather than fading.
    fn lasting(&self) -> bool {
        self.bad || self.stays || self.link.is_some()
    }

    /// A button dismissing it, saying `label`. Without one a lasting flash is dismissed by pressing it.
    pub fn dismiss(mut self, label: impl Into<Text<'a>>) -> Self {
        self.dismiss = label.into();
        self
    }
}

/// An id no earlier message had, in this process or an earlier one a tab outlived, so a patch makes each message a
/// new element: a morph would keep the last one, faded and hidden, and only change its words.
fn serial() -> String {
    format!("flash-{:016x}", crate::action::serial())
}

component!(Flash);

/// Where a list would be, when it has nothing: `kit::Empty::new("nothing downloading")`.
#[must_use = "an empty state draws nothing until it is rendered"]
#[derive(Clone, Debug, Template)]
#[template(path = "empty.html")]
pub struct Empty<'a> {
    title: Text<'a>,
    detail: Content<'a>,
    icon: Option<Glyph>,
    action: Option<Button<'a>>,
    compact: bool,
    attrs: Attrs<'a>,
}

impl<'a> Empty<'a> {
    /// Nothing here, said as `title`.
    pub fn new(title: impl Into<Text<'a>>) -> Self {
        Self {
            title: title.into(),
            detail: Content::default(),
            icon: None,
            action: None,
            compact: false,
            attrs: Attrs::default(),
        }
    }

    /// Why, or what would fill it.
    pub fn detail(mut self, detail: impl Into<Content<'a>>) -> Self {
        self.detail = detail.into();
        self
    }

    /// A glyph above the title.
    pub fn icon(mut self, glyph: Glyph) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// Small, for a list in a sidebar or a panel rather than a page.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// The action that would fill it.
    pub fn action(mut self, button: Button<'a>) -> Self {
        self.action = Some(button);
        self
    }
}

impl Wrap for Empty<'_> {
    fn wrap(mut self, content: Markup) -> Self {
        self.detail = content.into();
        self
    }
}

component!(Empty);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_notice_is_an_alert() {
        let notice = Notice::bad("The token was refused.").to_string();
        assert!(notice.contains("class=\"notice bad\""), "{notice}");
        assert!(notice.contains("role=\"alert\""), "{notice}");
        assert!(
            Notice::new("Saved.")
                .to_string()
                .contains("role=\"status\"")
        );
    }

    #[test]
    fn a_flash_replaces_the_region_it_is_patched_into() {
        let region = Flash::region().to_string();
        let flash = Flash::bad("<script>").to_string();
        assert!(region.contains("id=\"flash\""), "{region}");
        assert!(flash.contains("id=\"flash\""), "{flash}");
        assert!(flash.contains("&#60;script&#62;"), "{flash}");
        assert!(flash.contains("role=\"alert\""), "{flash}");
        assert!(
            region.contains(&format!("data-on:{}__window", Flash::EVENT)),
            "page script can raise one: {region}"
        );
    }

    #[test]
    fn a_flash_with_a_link_stays() {
        let flash = Flash::ok("Moved 3 images.")
            .link("/albums/2", "open that album")
            .to_string();
        assert!(flash.contains("<p class=\"toast\""), "{flash}");
        assert!(
            flash.contains("<a href=\"/albums/2\">open that album</a>"),
            "{flash}"
        );
        assert!(
            Flash::ok("Saved.")
                .stays()
                .to_string()
                .contains("<p class=\"toast\"")
        );
        assert!(
            Flash::ok("Saved.")
                .to_string()
                .contains("class=\"toast brief\">Saved.")
        );
    }
}

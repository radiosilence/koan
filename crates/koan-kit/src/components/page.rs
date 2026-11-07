//! A page: its header and its body, with the margins the page decides and
//! the content does not. The Mac's `PageHeader` and the page edges around
//! it; a page declares what it shows and nothing about where it sits.

use gpui::prelude::*;
use gpui::*;

use super::RowMetrics;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, TypeRole, label};

/// A title standing alone, outside a page's header: the About window's
/// name, a confirmation's question. `title`, `strong`, kept as given.
pub fn title_text(text: impl Into<SharedString>, look: &Look) -> Div {
    div()
        .text_role(TypeRole::Title, look)
        .text_color(look.palette.strong.hsla())
        .child(text.into())
}

/// The header every page shares: an optional cover, the title over a
/// detail line (a count, a duration), the page's actions under them, and
/// what trails at the far edge.
#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    art: Option<AnyElement>,
    lead: Vec<AnyElement>,
    detail: Option<AnyElement>,
    badges: Vec<super::Badge>,
    actions: Vec<AnyElement>,
    body: Option<AnyElement>,
    trailing: Option<AnyElement>,
}

impl PageHeader {
    /// A title in the app's words ("albums", "history"), drawn lowercase.
    pub fn new(title: impl AsRef<str>) -> Self {
        Self::data(label(title))
    }

    /// A title that is data (a record, an artist, a playlist), which keeps
    /// its case.
    pub fn data(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            art: None,
            lead: Vec::new(),
            detail: None,
            badges: Vec::new(),
            actions: Vec::new(),
            body: None,
            trailing: None,
        }
    }

    /// The cover or photo, leading, top-aligned with the title.
    pub fn art(mut self, art: impl IntoElement) -> Self {
        self.art = Some(art.into_any_element());
        self
    }

    /// Above the title, side by side: a record's `PlayButton` and
    /// `ShuffleButton`.
    pub fn lead(mut self, item: impl IntoElement) -> Self {
        self.lead.push(item.into_any_element());
        self
    }

    /// Under the title, in `control`, `muted`: a count, or a credit with its
    /// links.
    pub fn detail(mut self, detail: impl IntoElement) -> Self {
        self.detail = Some(detail.into_any_element());
        self
    }

    /// After the detail, on its line: what kind of thing the page shows, where
    /// that changes what can be done to it ("smart", "read-only").
    pub fn badge(mut self, badge: super::Badge) -> Self {
        self.badges.push(badge);
        self
    }

    /// Under the detail, side by side: the page's lesser verbs, compact
    /// buttons with their glyphs.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// Under the actions: a paragraph about the page's subject, an artist's
    /// biography.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// At the far edge: a filter field, sorts, select.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

/// The heading over one of a page's sections (an artist's albums, their
/// EPs and singles, the records they feature on), the Mac's: `body` in
/// `strong`, lowercase, with the room above and below it its own. A list
/// that measures its items takes `SectionHeading::height`.
#[derive(IntoElement)]
pub struct SectionHeading {
    title: SharedString,
    trailing: Vec<AnyElement>,
    count: Option<SharedString>,
    open: Option<(ElementId, super::Handler<ClickEvent>)>,
    reveal: Option<super::choice::Run>,
}

impl SectionHeading {
    pub fn new(title: impl AsRef<str>) -> Self {
        Self {
            title: label(title),
            trailing: Vec::new(),
            count: None,
            open: None,
            reveal: None,
        }
    }

    /// Run when a screen reader moves to the heading in a list that draws
    /// only what is on screen, as `TableRow::on_reveal` is for a row: the
    /// list scrolls the reader's next stop on screen and is drawn again.
    pub fn on_reveal(mut self, reveal: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.reveal = Some(std::rc::Rc::new(reveal));
        self
    }

    /// How many the section holds in all, in `muted` beside the title: a
    /// preview of the first few says how many there are.
    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// The whole heading opens what the section previews (a shelf's "see
    /// all"), a disclosure glyph after the count saying so.
    pub fn on_open(
        mut self,
        id: impl Into<ElementId>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.open = Some((id.into(), std::sync::Arc::new(handler)));
        self
    }

    /// Beside the title: the section's own play and queue.
    pub fn trailing(mut self, item: impl IntoElement) -> Self {
        self.trailing.push(item.into_any_element());
        self
    }

    /// Above the heading's line.
    fn above(look: &Look) -> Pixels {
        look.space(Space::L)
    }

    /// Between the heading's line and the section.
    fn below(look: &Look) -> Pixels {
        look.space(Space::M)
    }

    pub fn height(look: &Look) -> Pixels {
        Self::above(look) + look.line_height(TypeRole::Body) + Self::below(look)
    }
}

impl RenderOnce for SectionHeading {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let opens = self.open.is_some();
        let spoken = self.title.clone();
        let title = div()
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .text_role(TypeRole::Body, &look)
            .text_color(look.palette.strong.hsla())
            .child(self.title)
            .children(
                self.count
                    .map(|count| div().text_color(look.palette.muted.hsla()).child(count)),
            )
            .when(opens, |d| {
                d.child(
                    KoanIcon::new(Icon::Disclosure)
                        .role(TypeRole::Fine)
                        .color(look.palette.muted.hsla()),
                )
            });
        let title = match self.open {
            Some((id, open)) => div()
                .id(id)
                .cursor_pointer()
                .child(title)
                .on_click(move |event, window, cx| open(event, window, cx))
                .into_any_element(),
            None => title.into_any_element(),
        };
        let list_view = window.current_view();
        let heading = super::accessible(
            div().id(ElementId::Name(format!("section-heading-{spoken}").into())),
            gpui::Role::Heading,
            Some(spoken),
        );
        match self.reveal {
            Some(reveal) => heading.on_a11y_action(
                gpui::AccessibleAction::ScrollIntoView,
                move |_, window, cx| {
                    reveal(window, cx);
                    cx.notify(list_view);
                },
            ),
            None => heading,
        }
        .aria_level(2)
        .w_full()
        .h(Self::height(&look))
        .pt(Self::above(&look))
        .pb(Self::below(&look))
        .flex()
        .items_center()
        .gap(look.space(Space::L))
        .child(title)
        .children(self.trailing)
    }
}

impl RenderOnce for PageHeader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        // At a phone's width the text beside a cover has no room: the cover,
        // then the title and detail, then the controls in one row, as the
        // iOS app's record pages stack.
        if window.viewport_size().width < px(NARROW) {
            return self.stacked(&look).into_any_element();
        }
        // The whole row's width, whatever holds it: a list item sizes to its
        // content, and the text beside a cover would collapse to nothing.
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(look.spaced(18.0))
            .children(self.art)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(look.spaced(6.0))
                    .when(!self.lead.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(look.space(Space::M))
                                .pb(look.space(Space::S))
                                .children(self.lead),
                        )
                    })
                    .child({
                        // The page's heading to a screen reader.
                        let title = super::accessible(
                            div().id(ElementId::Name(format!("page-title-{}", self.title).into())),
                            gpui::Role::Heading,
                            Some(self.title.clone()),
                        )
                        .aria_level(1)
                        .text_role(TypeRole::Title, &look)
                        .text_color(p.strong.hsla())
                        .line_clamp(2)
                        .child(self.title);
                        #[cfg(test)]
                        let title = title.debug_selector(|| "page-header-title".into());
                        title
                    })
                    .when(self.detail.is_some() || !self.badges.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(look.space(Space::S))
                                .text_role(TypeRole::Control, &look)
                                .text_color(p.muted.hsla())
                                .children(self.detail)
                                .children(self.badges),
                        )
                    })
                    .when(!self.actions.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(look.space(Space::M))
                                .pt(look.space(Space::Xs))
                                .children(self.actions),
                        )
                    })
                    .children(
                        self.body
                            .map(|body| div().pt(look.space(Space::S)).child(body)),
                    ),
            )
            .children(self.trailing.map(|t| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(look.space(Space::L))
                    .child(t)
            }))
            .into_any_element()
    }
}

/// The width below which a page header stacks: a phone held upright.
const NARROW: f32 = 600.0;

impl PageHeader {
    fn stacked(self, look: &Look) -> impl IntoElement {
        let p = look.palette;
        let title = super::accessible(
            div().id(ElementId::Name(format!("page-title-{}", self.title).into())),
            gpui::Role::Heading,
            Some(self.title.clone()),
        )
        .aria_level(1)
        .text_role(TypeRole::Title, look)
        .text_color(p.strong.hsla())
        .line_clamp(3)
        .child(self.title);
        #[cfg(test)]
        let title = title.debug_selector(|| "page-header-title".into());
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(look.spaced(10.0))
            .children(self.art)
            .child(title)
            .when(self.detail.is_some() || !self.badges.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(look.space(Space::S))
                        .text_role(TypeRole::Control, look)
                        .text_color(p.muted.hsla())
                        .children(self.detail)
                        .children(self.badges),
                )
            })
            .when(!self.lead.is_empty() || !self.actions.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(look.space(Space::M))
                        .children(self.lead)
                        .children(self.actions),
                )
            })
            .children(self.body)
            .children(self.trailing)
    }
}

/// The header's play glyph, the Mac's 17 points at the chosen text size:
/// the square is the prominent action's, and its glyph fills it as one.
const PLAY_GLYPH: f32 = 17.0;
/// The shuffle glyph beside it, the Mac's 15 points.
const SHUFFLE_GLYPH: f32 = 15.0;

/// A header's play: the page's one prominent action, as the Mac draws it
/// in the theme. A square in a hairline accent outline with the play glyph
/// in the accent, 44 points to hit, and the spinner while what it plays is
/// still being read.
#[derive(IntoElement)]
pub struct PlayButton {
    id: ElementId,
    label: SharedString,
    loading: bool,
    disabled: bool,
    on_click: Option<super::Handler<ClickEvent>>,
}

impl PlayButton {
    /// `label` names it to VoiceOver ("play the record").
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            loading: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Nothing to play: dimmed, as `ShuffleButton::disabled`, and not
    /// clickable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(std::sync::Arc::new(handler));
        self
    }
}

impl RenderOnce for PlayButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let accent = look.color(crate::theme::Tone::Accent);
        let hover = look.palette.hover.hsla();
        let loading = self.loading;
        // Either way there is nothing to press.
        let idle = loading || self.disabled;
        // A press lifts the square's ground, as an outlined button's does.
        let answers = super::Answerer::of(&self.id, window, cx);
        let lifted = answers.layer(window, cx);
        let on_click = self.on_click.filter(|_| !idle).map(|on_click| {
            let (answers, accent) = (answers.clone(), look.accent.hsla());
            #[allow(clippy::arc_with_non_send_sync)] // handlers run on the main thread only
            let press: super::Handler<ClickEvent> = std::sync::Arc::new(move |e, window, cx| {
                answers.start(super::Answer::Lift, accent, cx);
                on_click(e, window, cx)
            });
            press
        });
        super::accessible(
            div().id(self.id),
            gpui::Role::Button,
            Some(self.label.clone()),
        )
        .relative()
        .children(lifted)
        .size(crate::theme::HIT)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .border(look.hairline)
        .border_color(accent)
        .aria_disabled(idle)
        .when(self.disabled, |d| d.opacity(crate::theme::DISABLED_OPACITY))
        .when(!idle, |d| d.cursor_pointer().active(move |s| s.bg(hover)))
        .child(if loading {
            super::Spinner::new(look.font_size(TypeRole::Body)).into_any_element()
        } else {
            crate::theme::KoanIcon::new(crate::theme::Icon::Play)
                .points(PLAY_GLYPH * look.scale.size)
                .color(look.text_color(crate::theme::Tone::Accent))
                .into_any_element()
        })
        .when_some(on_click, |d, on_click| {
            super::answers_click(d, on_click.clone())
                .on_click(move |event, window, cx| on_click(event, window, cx))
        })
    }
}

/// Beside the header's play and quieter than it: the same tracks shuffled.
/// The glyph in `muted`, 44 points to hit.
#[derive(IntoElement)]
pub struct ShuffleButton {
    id: ElementId,
    /// What it shuffles, for its tooltip: "shuffle Hounds of Love".
    name: SharedString,
    disabled: bool,
    on_click: Option<super::Handler<ClickEvent>>,
}

impl ShuffleButton {
    /// `name` is what it shuffles, a record's or a playlist's, kept in its
    /// own case.
    pub fn new(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            disabled: false,
            on_click: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(std::sync::Arc::new(handler));
        self
    }
}

impl RenderOnce for ShuffleButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ink = look.palette.ink.hsla();
        // A glyph draws in its own colour, so the hover reaches it through
        // the button's group rather than the text colour.
        let group = SharedString::from(format!("shuffle-{:?}", self.id));
        let disabled = self.disabled;
        let help: SharedString = format!("{} {}", crate::theme::label("shuffle"), self.name).into();
        // A press swings the arrows apart and crosses them again, as the
        // transport's shuffle turning on, drawn over the glyph it hides.
        let answers = super::Answerer::of(&self.id, window, cx);
        let points = SHUFFLE_GLYPH * look.scale.size;
        let color = if look.touch && !disabled {
            ink
        } else {
            look.color(crate::theme::Tone::Muted)
        };
        let hidden = answers.glyph_live(cx);
        let drawn = answers.glyph_layer(
            px(crate::theme::icons::side(points)),
            color,
            None,
            window,
            cx,
        );
        let on_click = self.on_click.filter(|_| !disabled).map(|on_click| {
            let (answers, accent) = (answers.clone(), look.accent.hsla());
            #[allow(clippy::arc_with_non_send_sync)] // handlers run on the main thread only
            let press: super::Handler<ClickEvent> = std::sync::Arc::new(move |e, window, cx| {
                answers.start(
                    super::Answer::Morph(super::answer::Morph::ShuffleOn),
                    accent,
                    cx,
                );
                on_click(e, window, cx)
            });
            press
        });
        super::accessible(div().id(self.id), gpui::Role::Button, Some(help.clone()))
            .group(group.clone())
            .aria_disabled(disabled)
            .size(crate::theme::HIT)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .when(disabled, |d| d.opacity(crate::theme::DISABLED_OPACITY))
            .when(!disabled, |d| d.cursor_pointer())
            .child(
                div()
                    .relative()
                    .child(
                        div().when(hidden, |d| d.invisible()).child(
                            crate::theme::KoanIcon::new(crate::theme::Icon::Shuffle)
                                .points(points)
                                .color(color)
                                .when(!disabled, |i| i.hover_color(group, ink)),
                        ),
                    )
                    .children(drawn),
            )
            .tooltip(move |window, cx| super::Tooltip::view(help.clone(), window, cx))
            .when_some(on_click, |d, on_click| {
                super::answers_click(d, on_click.clone())
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
    }
}

/// A page: the header at the page's margins, then the body, which fills the
/// rest and lays its own rows out at the same margins (`RowMetrics`).
#[derive(IntoElement)]
pub struct Page {
    header: Option<PageHeader>,
    body: Option<AnyElement>,
}

impl Page {
    pub fn new(header: PageHeader) -> Self {
        Self {
            header: Some(header),
            body: None,
        }
    }

    /// A page with no header: a grid that scrolls its header away with it.
    pub fn bare() -> Self {
        Self {
            header: None,
            body: None,
        }
    }

    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }
}

impl RenderOnce for Page {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        // A header held above the body sits clear of a phone's bar; a bare
        // page's body scrolls under it by its own lights.
        let under_bar = match self.header {
            Some(_) => super::bar_inset(window, cx),
            None => Pixels::ZERO,
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .pt(under_bar)
            .text_role(TypeRole::Body, &look)
            .text_color(look.palette.ink.hsla())
            .children(self.header.map(|header| head(header, &look).flex_none()))
            .child(div().relative().flex_1().min_h_0().children(self.body))
    }
}

/// A page's header at the page's margins: a `Page`'s own, or one a list
/// scrolls with its rows (`RowList::lead`).
pub fn head(header: PageHeader, look: &Look) -> Div {
    div()
        .px(RowMetrics::page_margin(look))
        .pt(RowMetrics::page_top(look))
        .pb(RowMetrics::header_gap(look))
        .child(header)
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext, VisualTestContext};

    use super::*;
    use crate::components::{Cover, CoverArt, FadeIn};
    use core::prelude::v1::test;

    struct Header {
        photo: bool,
    }

    impl Render for Header {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let side = RowMetrics::artist_photo(&Look::of(window, cx));
            let header = PageHeader::data("2manydjs").detail("12 albums");
            let header = if self.photo {
                header.art(FadeIn::new("photo", Cover::new(CoverArt::Missing, side)))
            } else {
                header
            };
            div().w(px(800.)).child(header)
        }
    }

    /// A photo arriving takes its square beside the title and nothing more:
    /// the title keeps the rest of the row, before the photo and after.
    #[gpui::test]
    fn a_photo_fading_in_leaves_the_title_its_room(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
        });
        let (header, cx) = cx.add_window_view(|_, _| Header { photo: false });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let before = cx.debug_bounds("page-header-title").expect("title drawn");
        assert!(before.size.width > px(600.), "{before:?}");
        header.update(cx, |h, cx| {
            h.photo = true;
            cx.notify();
        });
        cx.run_until_parked();
        let after = cx.debug_bounds("page-header-title").expect("title drawn");
        assert!(after.size.width > px(500.), "{after:?}");
        assert!(after.size.height < px(80.), "{after:?}");
        assert!(after.origin.x > before.origin.x);
    }
}

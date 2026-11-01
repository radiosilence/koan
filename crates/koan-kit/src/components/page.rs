//! A page: its header and its body, with the margins the page decides and
//! the content does not. The Mac's `PageHeader` and the page edges around
//! it; a page declares what it shows and nothing about where it sits.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::RowMetrics;
use crate::theme::{KoanStyled, Look, Space, TypeRole, label};

/// The header every page shares: an optional cover, the title over a
/// detail line (a count, a duration), the page's actions under them, and
/// what trails at the far edge.
#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    art: Option<AnyElement>,
    lead: Vec<AnyElement>,
    detail: Option<AnyElement>,
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

impl RenderOnce for PageHeader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
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
                        let title = div()
                            .text_role(TypeRole::Title, &look)
                            .text_color(p.strong.hsla())
                            .line_clamp(2)
                            .child(self.title);
                        #[cfg(test)]
                        let title = title.debug_selector(|| "page-header-title".into());
                        title
                    })
                    .children(self.detail.map(|detail| {
                        div()
                            .text_role(TypeRole::Control, &look)
                            .text_color(p.muted.hsla())
                            .child(detail)
                    }))
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
    }
}

/// A header's play: the page's one prominent action, as the Mac draws it
/// in the theme. A square in a hairline accent outline with the play glyph
/// in the accent, 44 points to hit, and the spinner while what it plays is
/// still being read.
#[derive(IntoElement)]
pub struct PlayButton {
    id: ElementId,
    label: SharedString,
    loading: bool,
    on_click: Option<super::Handler<ClickEvent>>,
}

impl PlayButton {
    /// `label` names it to VoiceOver ("play the record").
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            loading: false,
            on_click: None,
        }
    }

    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
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
        let _ = &self.label;
        div()
            .id(self.id)
            .size(crate::theme::HIT)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border(crate::theme::HAIRLINE)
            .border_color(accent)
            .when(!loading, |d| {
                d.cursor_pointer().active(move |s| s.bg(hover))
            })
            .child(if loading {
                super::Spinner::new(look.font_size(TypeRole::Body)).into_any_element()
            } else {
                crate::theme::KoanIcon::new(crate::theme::Icon::Play)
                    .role(TypeRole::Body)
                    .color(look.text_color(crate::theme::Tone::Accent))
                    .into_any_element()
            })
            .when_some(self.on_click.filter(|_| !loading), |d, on_click| {
                d.on_click(move |event, window, cx| on_click(event, window, cx))
            })
    }
}

/// Beside the header's play and quieter than it: the same tracks shuffled.
/// The glyph in `muted`, 44 points to hit.
#[derive(IntoElement)]
pub struct ShuffleButton {
    id: ElementId,
    disabled: bool,
    on_click: Option<super::Handler<ClickEvent>>,
}

impl ShuffleButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
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
        div()
            .id(self.id)
            .size(crate::theme::HIT)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_color(look.palette.muted.hsla())
            .when(self.disabled, |d| d.opacity(crate::theme::DISABLED_OPACITY))
            .when(!self.disabled, |d| {
                d.cursor_pointer().hover(move |s| s.text_color(ink))
            })
            .child(
                crate::theme::KoanIcon::new(crate::theme::Icon::Shuffle)
                    .role(TypeRole::Body)
                    .tone(crate::theme::Tone::Muted),
            )
            .when_some(self.on_click.filter(|_| !self.disabled), |d, on_click| {
                d.on_click(move |event, window, cx| on_click(event, window, cx))
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
        let margin = RowMetrics::page_margin(&look);
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_role(TypeRole::Body, &look)
            .text_color(look.palette.ink.hsla())
            .children(self.header.map(|header| {
                div()
                    .flex_none()
                    .px(margin)
                    .pt(RowMetrics::page_top(&look))
                    .pb(RowMetrics::header_gap(&look))
                    .child(header)
            }))
            .child(div().relative().flex_1().min_h_0().children(self.body))
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Context, Render, TestAppContext, VisualTestContext};

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
    #[gpui_kit::test]
    fn a_photo_fading_in_leaves_the_title_its_room(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
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

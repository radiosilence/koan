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
    lead: Option<AnyElement>,
    detail: Option<SharedString>,
    actions: Option<AnyElement>,
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
            lead: None,
            detail: None,
            actions: None,
            trailing: None,
        }
    }

    /// The cover or photo, leading, top-aligned with the title.
    pub fn art(mut self, art: impl IntoElement) -> Self {
        self.art = Some(art.into_any_element());
        self
    }

    /// Above the title: a record's play and shuffle.
    pub fn lead(mut self, lead: impl IntoElement) -> Self {
        self.lead = Some(lead.into_any_element());
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Under the detail: the page's verbs.
    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
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
        div()
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
                    .children(self.lead.map(|lead| {
                        div()
                            .flex()
                            .items_center()
                            .gap(look.space(Space::M))
                            .pb(look.space(Space::S))
                            .child(lead)
                    }))
                    .child(
                        div()
                            .text_role(TypeRole::Title, &look)
                            .text_color(p.strong.hsla())
                            .line_clamp(2)
                            .child(self.title),
                    )
                    .children(self.detail.map(|detail| {
                        div()
                            .text_role(TypeRole::Control, &look)
                            .text_color(p.muted.hsla())
                            .child(detail)
                    }))
                    .children(
                        self.actions
                            .map(|actions| div().pt(look.space(Space::Xs)).child(actions)),
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

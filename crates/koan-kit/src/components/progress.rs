//! Something under way, as a row: what it is, how far it has come in counts,
//! a line of detail, and a bar that fills as it goes or, with no measure,
//! runs a segment along it; a stop button where it can be stopped. The row
//! decides its own spacing, room below the bar included, so a list of them
//! at a sidebar's foot never sits on the rule beneath.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Handler;
use crate::theme::{Icon, KoanIcon, KoanStyled, KoanTheme, Look, Motion, Space, TypeRole};

/// The bar's thickness.
const BAR: f32 = 2.0;
/// One run of an unmeasured bar's segment along it.
const RUN: Duration = Duration::from_millis(1200);

/// A bar on its own: `fraction` from 0 to 1, or `None` while there is no
/// measure, when a segment runs along it (still with Reduce Motion).
#[derive(IntoElement)]
pub struct ProgressBar {
    id: ElementId,
    fraction: Option<f32>,
}

impl ProgressBar {
    pub fn new(id: impl Into<ElementId>, fraction: Option<f32>) -> Self {
        Self {
            id: id.into(),
            fraction,
        }
    }
}

impl RenderOnce for ProgressBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let accent = look.color(crate::theme::Tone::Accent);
        let track = div()
            .w_full()
            .h(px(BAR))
            .bg(look.palette.rule.hsla())
            .relative()
            .overflow_hidden();
        match self.fraction {
            Some(fraction) => track
                .child(
                    div()
                        .h_full()
                        .w(relative(fraction.clamp(0.0, 1.0)))
                        .bg(accent),
                )
                .into_any_element(),
            None if KoanTheme::motion_allowed(cx) => track
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .h_full()
                        .w(relative(0.3))
                        .bg(accent)
                        .with_animation(self.id, Animation::new(RUN).repeat(), |segment, t| {
                            segment.left(relative(-0.3 + 1.3 * Motion::ease_in_out(t)))
                        }),
                )
                .into_any_element(),
            None => track
                .child(div().h_full().w_full().bg(accent.opacity(0.4)))
                .into_any_element(),
        }
    }
}

#[derive(IntoElement)]
pub struct ProgressRow {
    id: ElementId,
    title: SharedString,
    counts: Option<SharedString>,
    detail: Option<SharedString>,
    fraction: Option<f32>,
    on_stop: Option<Handler<ClickEvent>>,
}

impl ProgressRow {
    /// `title` says what is under way ("scanning the library").
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            counts: None,
            detail: None,
            fraction: None,
            on_stop: None,
        }
    }

    /// How far it has come, in its own units ("1,204 of 5,000 files").
    pub fn counts(mut self, counts: impl Into<SharedString>) -> Self {
        self.counts = Some(counts.into());
        self
    }

    /// Under the bar: the file or record it is on.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// From 0 to 1; left unset, the bar runs a segment.
    pub fn fraction(mut self, fraction: Option<f32>) -> Self {
        self.fraction = fraction;
        self
    }

    /// A stop button at the bar's end.
    pub fn on_stop(mut self, stop: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_stop = Some(std::sync::Arc::new(stop));
        self
    }
}

impl RenderOnce for ProgressRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let muted = look.palette.muted.hsla();
        // Every line truncates within the row: counts alone can outrun a
        // sidebar's width.
        let line = |text: SharedString, role: TypeRole, color: Hsla| {
            div()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_role(role, &look)
                .text_color(color)
                .child(text)
        };
        let stop = self.on_stop.map(|stop| {
            div()
                .id((self.id.clone(), "stop"))
                .flex_none()
                .cursor_pointer()
                .text_color(muted.opacity(0.6))
                .hover(move |s| s.text_color(muted))
                .child(
                    KoanIcon::new(Icon::Deselect)
                        .role(TypeRole::Fine)
                        .color(muted),
                )
                .on_click(move |e, window, cx| stop(e, window, cx))
        });
        div()
            .id(self.id.clone())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(look.spaced(3.0))
            .px(look.space(Space::L))
            .pt(look.space(Space::S))
            .pb(look.space(Space::M))
            .child(line(self.title, TypeRole::Fine, muted))
            .children(
                self.counts
                    .map(|c| line(c, TypeRole::Meta, muted.opacity(0.7))),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::S))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(ProgressBar::new((self.id, "bar"), self.fraction)),
                    )
                    .children(stop),
            )
            .children(
                self.detail
                    .map(|d| line(d, TypeRole::Fine, muted.opacity(0.7))),
            )
    }
}

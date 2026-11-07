use gpui::{
    App, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Size,
    Styled, Window, canvas, div, px,
};

use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// The width, in points at 100 % text, under which an empty state starts to
/// shrink: room for a title of some twenty-five characters inside its padding.
const ROOM: f32 = 280.0;

/// The least an empty state is drawn at. Smaller areas clip it.
const LEAST: f32 = 0.5;

/// An empty page: the place's glyph in `muted`, a title in `body`, `ink`,
/// and what to do about it in `meta`, `muted`, centred. In an area too small
/// for it (a narrow column, the lyrics beside a small window) it is drawn
/// smaller, down to half size, and past that clipped to the area rather than
/// drawn over what is beside it.
#[derive(IntoElement)]
pub struct EmptyState {
    id: Option<gpui::ElementId>,
    icon: Icon,
    title: SharedString,
    detail: Option<SharedString>,
    tone: Tone,
}

impl EmptyState {
    pub fn new(icon: Icon, title: impl AsRef<str>) -> Self {
        Self {
            id: None,
            icon,
            title: label(title),
            detail: None,
            tone: Tone::Muted,
        }
    }

    /// Something went wrong rather than nothing being there: the glyph in
    /// `bad`, as the Mac's startup error draws its warning.
    pub fn warning(mut self) -> Self {
        self.tone = Tone::Bad;
        self
    }

    /// The id it keeps its measured area under, its title unless given.
    /// Two empty states with one title side by side each need their own,
    /// or they share one area.
    pub fn id(mut self, id: impl Into<gpui::ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// A sentence, which keeps its case.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The scale to draw at in an area of `area` (what the foot leaves), given
/// that drawn at `scale` the content stood `content` tall, with text at
/// `text` times its points. The height asks for the scale at which the
/// content would just fit; in a parent sized by its content the area is the
/// content, which asks for the scale it has, so it holds rather than
/// shrinking frame after frame.
fn fit(area: Size<Pixels>, content: Pixels, scale: f32, text: f32) -> f32 {
    let wide = f32::from(area.width) / (ROOM * text);
    let content = f32::from(content);
    let tall = if content > 0.0 {
        f32::from(area.height) * scale / content
    } else {
        1.0
    };
    wide.min(tall).clamp(LEAST, 1.0)
}

/// The scale an empty state draws at, and how tall its content stood at it.
#[derive(Clone, Copy)]
struct Fit {
    scale: f32,
    content: Pixels,
}

impl RenderOnce for EmptyState {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let full = Look::of(window, cx);
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| gpui::ElementId::Name(format!("empty-{}", self.title).into()));
        let foot = super::foot_inset(window, cx);
        // As the last frame laid it out: the scale follows the area a frame
        // after it changes, and the first frame draws at full size.
        let fit_state = window.use_keyed_state(super::child(&id, "fit"), cx, |_, _| Fit {
            scale: 1.0,
            content: px(0.0),
        });
        let scale = fit_state.read(cx).scale;
        let mut look = full;
        look.scale.text *= scale;
        look.scale.space *= scale;
        let text = full.scale.text;
        let measured = fit_state.clone();
        // A group to a screen reader, named by what it says.
        let spoken: gpui::SharedString = match &self.detail {
            Some(detail) => format!("{}. {detail}", self.title).into(),
            None => self.title.clone(),
        };
        super::accessible(div().id(id), gpui::Role::Group, Some(spoken))
            .relative()
            .size_full()
            .overflow_hidden()
            // Centred in what a phone's foot leaves of the page.
            .pb(foot)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w_full()
                    .min_w_0()
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                measured.update(cx, |fit, _| fit.content = bounds.size.height)
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(look.space(Space::M))
                    .p(look.space(Space::Xxl))
                    // Room for a title on one line, and a sentence of detail on
                    // one or two with no word left alone on the last, at the size
                    // type is drawn.
                    .max_w(px(480.0 * look.scale.text))
                    .text_center()
                    .child(
                        KoanIcon::new(self.icon)
                            .points(28.0 * look.scale.text)
                            .tone(self.tone),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_role(TypeRole::Body, &look)
                            .text_color(look.palette.ink.hsla())
                            .child(self.title),
                    )
                    .children(self.detail.map(|detail| {
                        div()
                            .w_full()
                            .text_role(TypeRole::Meta, &look)
                            .text_color(look.palette.muted.hsla())
                            .child(detail)
                    })),
            )
            // After the content, so its height is known here.
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let area = Size {
                            width: bounds.size.width,
                            height: px(f32::from(bounds.size.height - foot).max(0.0)),
                        };
                        let now = *fit_state.read(cx);
                        let next = fit(area, now.content, now.scale, text);
                        if (next - now.scale).abs() > 0.005 {
                            fit_state.update(cx, |fit, _| fit.scale = next);
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: f32 = 0.86;

    fn size(width: f32, height: f32) -> Size<Pixels> {
        Size {
            width: px(width),
            height: px(height),
        }
    }

    #[test]
    fn full_size_with_room() {
        assert_eq!(fit(size(800.0, 600.0), px(200.0), 1.0, TEXT), 1.0);
    }

    #[test]
    fn shrinks_with_a_narrow_or_short_area() {
        let room = ROOM * TEXT;
        assert!((fit(size(room * 0.75, 600.0), px(200.0), 1.0, TEXT) - 0.75).abs() < 1e-4);
        assert!((fit(size(800.0, 160.0), px(200.0), 1.0, TEXT) - 0.8).abs() < 1e-4);
        // Grows back as the area does.
        assert_eq!(fit(size(800.0, 600.0), px(100.0), 0.5, TEXT), 1.0);
    }

    /// In a parent sized by its content the area is the content: the scale
    /// holds at full size, frame after frame, rather than ratcheting down.
    #[test]
    fn holds_in_a_parent_sized_by_its_content() {
        let mut scale = 1.0;
        for _ in 0..20 {
            let content = 180.0 * scale;
            scale = fit(size(400.0, content), px(content), scale, TEXT);
        }
        assert_eq!(scale, 1.0);
    }

    #[test]
    fn never_below_half() {
        assert_eq!(fit(size(20.0, 20.0), px(200.0), 1.0, TEXT), LEAST);
    }
}

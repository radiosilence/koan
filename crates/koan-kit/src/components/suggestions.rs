//! What a search field suggests as it is typed into: a square panel laid
//! under the field and over what follows it, headed groups of lines, each
//! line a sleeve or a glyph, a title and a line of detail. One line is lit
//! at a time, by the arrows or the pointer, on `surface`.
//!
//! The panel is placed by its parent: it hangs from the bottom of the
//! nearest `relative` element and paints above everything else in the
//! window. What lighting and taking a line do is the owner's.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::RowMetrics;
use crate::theme::{HAIRLINE, Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

#[derive(IntoElement)]
pub struct SuggestionPanel {
    base: Stateful<Div>,
    children: Vec<AnyElement>,
}

impl SuggestionPanel {
    /// The tallest the panel grows before it scrolls, at 100 %.
    const MAX_HEIGHT: f32 = 560.0;

    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(id),
            children: Vec::new(),
        }
    }
}

impl ParentElement for SuggestionPanel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl InteractiveElement for SuggestionPanel {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for SuggestionPanel {}

impl RenderOnce for SuggestionPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let inset = look.space(Space::S);
        deferred(
            self.base
                .absolute()
                .top(relative(1.))
                .left(inset)
                .right(inset)
                .max_h(look.spaced(Self::MAX_HEIGHT))
                .overflow_y_scroll()
                .occlude()
                .flex()
                .flex_col()
                .pb(look.space(Space::Xs))
                .bg(look.palette.bg.hsla())
                .border(look.hairline)
                .border_color(look.palette.rule.hsla())
                .children(self.children),
        )
        .with_priority(1)
    }
}

/// The name over a group of lines: "tracks", "albums".
#[derive(IntoElement)]
pub struct SuggestionHeading {
    title: SharedString,
}

impl SuggestionHeading {
    pub fn new(title: impl AsRef<str>) -> Self {
        Self {
            title: label(title),
        }
    }
}

impl RenderOnce for SuggestionHeading {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .px(look.space(Space::S))
            .pt(look.space(Space::S))
            .pb(look.space(Space::Xs))
            .text_role(TypeRole::Fine, &look)
            .text_color(look.palette.muted.hsla())
            .child(self.title)
    }
}

/// One suggestion: its art, its title in `meta`, `ink`, and a line of
/// detail in `fine`, `muted`, both truncating. Listeners attach as to a
/// `div`.
#[derive(IntoElement)]
pub struct SuggestionRow {
    base: Stateful<Div>,
    title: SharedString,
    detail: Option<SharedString>,
    art: Option<AnyElement>,
    icon: Option<Icon>,
    lit: bool,
    picked: Option<bool>,
    figure: Option<SharedString>,
}

impl SuggestionRow {
    /// A suggestion names a thing, so its title keeps its case.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            base: div().id(id),
            title: title.into(),
            detail: None,
            art: None,
            icon: None,
            lit: false,
            picked: None,
            figure: None,
        }
    }

    /// In a list things are picked from (the picker): the tick in the art's
    /// place, in the accent once picked.
    pub fn picked(mut self, picked: bool) -> Self {
        self.picked = Some(picked);
        self
    }

    /// Trailing, in `meta`, `muted`: a track's length.
    pub fn figure(mut self, figure: impl Into<SharedString>) -> Self {
        self.figure = Some(figure.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The thing's own face, `RowMetrics::sleeve` square: a record's sleeve.
    pub fn art(mut self, art: impl IntoElement) -> Self {
        self.art = Some(art.into_any_element());
        self
    }

    /// A glyph in the art's place, for a thing with no face: an artist.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn lit(mut self, lit: bool) -> Self {
        self.lit = lit;
        self
    }
}

impl InteractiveElement for SuggestionRow {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for SuggestionRow {}

impl RenderOnce for SuggestionRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let side = RowMetrics::sleeve(&look);
        let line = |text: SharedString, role: TypeRole, color: Hsla| {
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_role(role, &look)
                .text_color(color)
                .child(text)
        };
        let art = match (self.picked, self.art, self.icon) {
            (Some(picked), ..) => div()
                .size(side)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(if picked {
                    KoanIcon::new(Icon::Picked)
                        .role(TypeRole::Body)
                        .color(look.accent.hsla())
                } else {
                    KoanIcon::new(self.icon.unwrap_or(Icon::Unpicked))
                        .role(TypeRole::Body)
                        .tone(Tone::Muted)
                }),
            (None, Some(art), _) => div().size(side).flex_none().child(art),
            (None, None, icon) => div()
                .size(side)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .children(
                    icon.map(|icon| KoanIcon::new(icon).role(TypeRole::Meta).tone(Tone::Muted)),
                ),
        };
        self.base
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .px(look.space(Space::S))
            .py(look.space(Space::Xs))
            .cursor_pointer()
            .when(self.lit, |d| d.bg(look.palette.surface.hsla()))
            .child(art)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(line(self.title, TypeRole::Meta, look.palette.ink.hsla()))
                    .children(
                        self.detail
                            .map(|d| line(d, TypeRole::Fine, look.palette.muted.hsla())),
                    ),
            )
            .children(self.figure.map(|figure| {
                div()
                    .ml_auto()
                    .flex_none()
                    .text_role(TypeRole::Meta, &look)
                    .text_color(look.palette.muted.hsla())
                    .child(figure)
            }))
    }
}

//! A signal chain drawn down the page: where it starts and ends
//! (`ChainEnd`), the stages it passes through (`ChainStage`, and
//! `ChainSlot` for one not filled yet), and the line joining them
//! (`ChainLink`, `ChainJoin` with words along it). The line is drawn in the
//! accent while the chain is lit (a match, say) and as a rule otherwise.
//! kōan's EQ page draws a device's chain with it.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div,
};

use super::Handler;
use super::sheet::note;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, MARK, Space, Tone, TypeRole};

/// The line's distance from the chain's leading edge, in points on the
/// spacing scale: under the middle of an end's glyph.
const LINE_AT: f32 = 18.0;
/// An end's indent, in points on the spacing scale, which puts its glyph
/// over the line.
const END_INDENT: f32 = 8.0;

fn line_colour(lit: bool, look: &Look) -> gpui::Hsla {
    if lit {
        look.accent.hsla()
    } else {
        look.palette.rule.hsla()
    }
}

fn line_width(lit: bool, look: &Look) -> gpui::Pixels {
    if lit { MARK } else { look.hairline }
}

/// A step of the line between two parts of the chain.
#[derive(IntoElement)]
pub struct ChainLink {
    lit: bool,
}

impl ChainLink {
    /// A link, in the accent when `lit`.
    pub fn new(lit: bool) -> Self {
        Self { lit }
    }
}

impl RenderOnce for ChainLink {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .ml(look.spaced(LINE_AT))
            .w(line_width(self.lit, &look))
            .h(look.space(Space::M))
            .bg(line_colour(self.lit, &look))
    }
}

/// The line between two stages with what is said about the join beside it:
/// at least a link's height, as tall as its lines.
#[derive(IntoElement)]
pub struct ChainJoin {
    lit: bool,
    lines: Vec<AnyElement>,
}

impl ChainJoin {
    /// A join, its line in the accent when `lit`.
    pub fn new(lit: bool) -> Self {
        Self {
            lit,
            lines: Vec::new(),
        }
    }
}

impl ParentElement for ChainJoin {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.lines.extend(elements)
    }
}

impl RenderOnce for ChainJoin {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(look.space(Space::Xs))
            .py(look.space(Space::Xs))
            .min_h(look.space(Space::M))
            .pl(look.spaced(LINE_AT) + look.space(Space::M))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(look.spaced(LINE_AT))
                    .w(line_width(self.lit, &look))
                    .bg(line_colour(self.lit, &look)),
            )
            .children(self.lines)
    }
}

/// Where the chain starts or ends: a glyph over the line and its name.
#[derive(IntoElement)]
pub struct ChainEnd {
    icon: Icon,
    title: SharedString,
}

impl ChainEnd {
    /// An end named `title`, its glyph `icon` drawn when icons are shown.
    pub fn new(icon: Icon, title: impl Into<SharedString>) -> Self {
        Self {
            icon,
            title: title.into(),
        }
    }
}

impl RenderOnce for ChainEnd {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .pl(look.spaced(END_INDENT))
            .text_role(TypeRole::Meta, &look)
            .text_color(look.text_color(Tone::Muted))
            .when(look.icons, |d| {
                d.child(KoanIcon::new(self.icon).role(TypeRole::Meta).tone(Tone::Muted))
            })
            .child(self.title)
    }
}

/// A stage of the chain, outlined: its title, what fills it and a detail,
/// a picture of what it does, and its controls. Pressing the words or the
/// picture chooses what fills it. Below the compact width the words take
/// the whole width, over a row of the picture and the controls; beside them
/// they would be cut to a word.
#[derive(IntoElement)]
pub struct ChainStage {
    id: ElementId,
    title: SharedString,
    name: SharedString,
    detail: Option<SharedString>,
    art: Option<AnyElement>,
    controls: Vec<AnyElement>,
    lit: bool,
    on_click: Option<Handler<ClickEvent>>,
}

impl ChainStage {
    /// A stage titled `title`, filled by `name`.
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        name: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            name: name.into(),
            detail: None,
            art: None,
            controls: Vec::new(),
            lit: false,
            on_click: None,
        }
    }

    /// A line under the name; an empty one is left out.
    pub fn detail(mut self, detail: Option<impl Into<SharedString>>) -> Self {
        self.detail = detail.map(Into::into).filter(|d| !d.is_empty());
        self
    }

    /// What the stage does, drawn small beside its words.
    pub fn art(mut self, art: Option<impl IntoElement>) -> Self {
        self.art = art.map(IntoElement::into_any_element);
        self
    }

    /// A control at the stage's trailing edge.
    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.controls.push(control.into_any_element());
        self
    }

    /// Outlined in the accent, as the line is when the chain is lit.
    pub fn lit(mut self, lit: bool) -> Self {
        self.lit = lit;
        self
    }

    /// What pressing the stage's words does.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(std::sync::Arc::new(handler));
        self
    }
}

impl RenderOnce for ChainStage {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let compact = look.compact;
        let words = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(look.space(Space::Xxs))
            .child(note(self.title, Tone::Muted, &look))
            .child(
                div()
                    .text_role(TypeRole::Body, &look)
                    .text_color(look.palette.ink.hsla())
                    .truncate()
                    .child(self.name),
            )
            .children(
                self.detail
                    .map(|d| note(d, Tone::Muted, &look).when(!compact, |n| n.truncate())),
            );
        let on_click = self.on_click;
        let pressable = div()
            .id(self.id)
            .cursor_pointer()
            .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)));
        let card = div()
            .flex()
            .gap(look.space(Space::M))
            .p(look.space(Space::S))
            .border(look.hairline)
            .border_color(line_colour(self.lit, &look));
        if compact {
            return card
                .flex_col()
                .child(pressable.child(words))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.space(Space::M))
                        .children(self.art)
                        .child(div().flex_1())
                        .children(self.controls),
                )
                .into_any_element();
        }
        card.items_center()
            .child(
                pressable
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::M))
                    .child(words)
                    .children(self.art),
            )
            .children(self.controls)
            .into_any_element()
    }
}

/// A stage not filled yet: a dashed place, with its title and a prompt to
/// choose what fills it, which pressing does.
#[derive(IntoElement)]
pub struct ChainSlot {
    id: ElementId,
    title: Option<SharedString>,
    prompt: SharedString,
    lit: bool,
    on_click: Option<Handler<ClickEvent>>,
}

impl ChainSlot {
    /// A slot asking for what fills it with `prompt`.
    pub fn new(id: impl Into<ElementId>, prompt: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: None,
            prompt: prompt.into(),
            lit: false,
            on_click: None,
        }
    }

    /// The stage's title, over the prompt.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Outlined in the accent, as the line is when the chain is lit.
    pub fn lit(mut self, lit: bool) -> Self {
        self.lit = lit;
        self
    }

    /// What pressing the slot does.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(std::sync::Arc::new(handler));
        self
    }
}

impl RenderOnce for ChainSlot {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let on_click = self.on_click;
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .gap(look.space(Space::Xxs))
            .p(look.space(Space::S))
            .border(look.hairline)
            .border_dashed()
            .border_color(if self.lit {
                look.accent.hsla()
            } else {
                look.palette.muted.hsla()
            })
            .cursor_pointer()
            .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
            .children(self.title.map(|t| note(t, Tone::Muted, &look)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::Xs))
                    .text_role(TypeRole::Body, &look)
                    .text_color(look.text_color(Tone::Accent))
                    .when(look.icons, |d| {
                        d.child(KoanIcon::new(Icon::Add).role(TypeRole::Body).tone(Tone::Accent))
                    })
                    .child(self.prompt),
            )
    }
}

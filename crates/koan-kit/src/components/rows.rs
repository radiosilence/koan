use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, InteractiveElement, Interactivity, IntoElement,
    ParentElement, RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Window,
    div,
};

use super::{Handler, accessible, answers_click, child, fade, fill, transition};
use crate::theme::{HIT, Icon, KoanIcon, KoanStyled, Look, MARK, Space, TypeRole, label};

/// The hover group a nav row's glyph brightens by.
const NAV_ROW: &str = "koan-nav-row";

/// A navigation row, as the sidebar's: the label in `body`, `muted`, after
/// its glyph when icons are on; where you are, in the accent with a 2-point
/// accent rule on the leading edge, and no fill.
///
/// A place with a face of its own (a playlist) carries its art before the
/// label and a line of detail under it. Listeners attach as to a `div`, so
/// a row can take drops.
#[derive(IntoElement)]
pub struct NavRow {
    id: ElementId,
    base: Stateful<Div>,
    label: SharedString,
    icon: Option<Icon>,
    art: Option<AnyElement>,
    detail: Option<SharedString>,
    trailing: Option<SharedString>,
    selected: bool,
    /// Under the keyboard's cursor while its list holds the keyboard.
    cursor: bool,
    spot: Option<super::CursorSpot>,
    on_click: Option<Handler<ClickEvent>>,
    accepts: Vec<Accept>,
    arrivals: Option<u64>,
}

/// A drop a row takes, attached as the row draws so the drop answers.
type Accept = Box<dyn FnOnce(Stateful<Div>, super::Answerer, gpui_kit::Hsla) -> Stateful<Div>>;

impl InteractiveElement for NavRow {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for NavRow {}

impl NavRow {
    pub fn new(id: impl Into<ElementId>, text: impl AsRef<str>) -> Self {
        Self::data(id, label(text))
    }

    /// A place named by data (a playlist), which keeps its case.
    pub fn data(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        let id = id.into();
        Self {
            base: div().id(id.clone()),
            id,
            label: text.into(),
            icon: None,
            art: None,
            detail: None,
            trailing: None,
            selected: false,
            cursor: false,
            spot: None,
            on_click: None,
            accepts: Vec::new(),
            arrivals: None,
        }
    }

    /// How many times something has arrived at the place (music queued or
    /// added to a playlist from a menu): each new arrival seen while the
    /// row is drawn answers as a drop does.
    pub fn arrivals(mut self, count: u64) -> Self {
        self.arrivals = Some(count);
        self
    }

    /// Something dropped on the row (tracks on the queue, a record on a
    /// playlist): the row answers with a line drawn under it, so the person
    /// sees where it went, then `handler` takes it.
    pub fn accept<T: 'static>(self, handler: impl Fn(&T, &mut Window, &mut App) + 'static) -> Self {
        self.accept_if(|_: &T| true, handler)
    }

    /// `accept`, for what `takes` says the row takes: a drop it does not
    /// take is let go without the answer saying it arrived.
    pub fn accept_if<T: 'static>(
        mut self,
        takes: impl Fn(&T) -> bool + 'static,
        handler: impl Fn(&T, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.accepts.push(Box::new(move |base, answers, accent| {
            base.on_drop(move |dropped: &T, window, cx| {
                if takes(dropped) {
                    answers.start(super::Answer::Sweep, accent, cx);
                    handler(dropped, window, cx)
                }
            })
        }));
        self
    }

    /// Where the cursor's ring tells its page it was laid out, while this
    /// is under the cursor: what the page's scroll area follows and the menu
    /// key opens under.
    pub fn spot(mut self, spot: &super::CursorSpot) -> Self {
        self.spot = Some(spot.clone());
        self
    }

    /// Under the keyboard's cursor: its list holds the keyboard and the
    /// arrows have come to it. Drawn with the focus ring while the keyboard
    /// was used last.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// The place's own face, square, before the label: a playlist's sleeve.
    /// Drawn whatever "show icons" says, as covers are.
    pub fn art(mut self, art: impl IntoElement) -> Self {
        self.art = Some(art.into_any_element());
        self
    }

    /// A line under the label, in `meta`, `muted`: a playlist's count.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A figure at the trailing edge, in `fine`, `muted`: how many
    /// downloads are running. Only while there is one to say.
    pub fn trailing(mut self, figure: impl Into<SharedString>) -> Self {
        self.trailing = Some(figure.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for NavRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let color = fade(
            child(&self.id, "selected"),
            self.selected,
            look.palette.muted.hsla(),
            look.text_color(crate::theme::Tone::Accent),
            window,
            cx,
        );
        let mark = fade(
            child(&self.id, "mark"),
            self.selected,
            look.accent.hsla().opacity(0.0),
            look.accent.hsla(),
            window,
            cx,
        );
        let on_click = self.on_click;
        let answers = super::Answerer::of(&self.id, window, cx);
        let accent = look.accent.hsla();
        let mut base = self.base;
        for accept in self.accepts {
            base = accept(base, answers.clone(), accent);
        }
        if let Some(count) = self.arrivals {
            answers.follow(count, super::Answer::Sweep, accent, window, cx);
        }
        let ink = look.palette.ink.hsla();
        let name = self.label.clone();
        let label = div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(self.label);
        let selected = self.selected;
        // A link to a screen reader: to its place, selected where it is.
        accessible(base, gpui_kit::Role::Link, Some(name))
            .aria_selected(selected)
            .group(NAV_ROW)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(look.space(Space::S))
            .px(look.space(Space::L))
            .h(super::RowMetrics::nav_height(self.detail.is_some(), &look))
            .text_role(TypeRole::Body, &look)
            .text_color(color)
            .when(!self.selected, move |row| {
                row.hover(move |s| s.text_color(ink))
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(MARK)
                    .bg(mark),
            )
            .children(
                self.icon
                    .filter(|_| look.icons && self.art.is_none())
                    // A glyph draws in its own colour: it brightens with the
                    // label under the pointer by the row's group.
                    .map(|icon| {
                        let glyph = KoanIcon::new(icon).color(color);
                        if selected {
                            glyph
                        } else {
                            glyph.hover_color(NAV_ROW, ink)
                        }
                    }),
            )
            .children(self.art)
            .map(|row| match self.detail {
                None => row.child(label),
                Some(detail) => row.child(
                    div().min_w_0().flex().flex_col().child(label).child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_role(TypeRole::Meta, &look)
                            .text_color(look.palette.muted.hsla())
                            .child(detail),
                    ),
                ),
            })
            .children(self.trailing.map(|figure| {
                div()
                    .ml_auto()
                    .flex_none()
                    .text_role(TypeRole::Fine, &look)
                    .text_color(look.palette.muted.hsla())
                    .child(figure)
            }))
            .children(answers.layer(window, cx))
            .when_some(on_click, |row, on_click| {
                answers_click(row, on_click.clone())
                    .cursor_pointer()
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            })
            .children(
                (self.cursor && window.last_input_was_keyboard())
                    .then(|| super::cursor_ring(&look, self.spot.as_ref())),
            )
    }
}

/// A heading over a group of navigation rows: `fine`, `ink`, lowercase,
/// with 16 points above, at the rows' inset.
#[derive(IntoElement)]
pub struct NavHeading {
    title: SharedString,
    trailing: Option<AnyElement>,
}

impl NavHeading {
    pub fn new(title: impl AsRef<str>) -> Self {
        Self {
            title: label(title),
            trailing: None,
        }
    }

    /// At the heading's far end: an action on the group ("new").
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl RenderOnce for NavHeading {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        accessible(
            div().id(ElementId::Name(
                format!("nav-heading-{}", self.title).into(),
            )),
            gpui_kit::Role::Heading,
            Some(self.title.clone()),
        )
        .aria_level(2)
        .flex()
        .flex_none()
        .items_end()
        .justify_between()
        .pt(look.space(Space::L))
        .pb(look.space(Space::Xs))
        .px(look.space(Space::L))
        .text_role(TypeRole::Fine, &look)
        .text_color(look.palette.ink.hsla())
        .child(self.title)
        .children(self.trailing)
    }
}

/// A line at the foot of a navigation list: what the app is doing, or what
/// went wrong. One line, ended in an ellipsis where it runs past the list,
/// so a long path in an error never widens or lengthens the foot.
#[derive(IntoElement)]
pub struct NavNote {
    text: SharedString,
    tone: crate::theme::Tone,
    said: Option<(ElementId, u64, bool)>,
}

impl NavNote {
    pub fn new(text: impl Into<SharedString>, tone: crate::theme::Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            said: None,
        }
    }

    /// A line the app says things in, under `id` whatever it says: each new
    /// thing said (`count` moving), a link copied, answers with a line
    /// drawn under it as it appears, so a notice replacing one like it is
    /// seen to. `fresh` while it has only just been said: drawn fresh then
    /// (the line appearing), it answers; drawn again later (its sidebar
    /// shown again), it does not answer an old notice.
    pub fn said(mut self, id: impl Into<ElementId>, count: u64, fresh: bool) -> Self {
        self.said = Some((id.into(), count, fresh));
        self
    }
}

impl RenderOnce for NavNote {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        // A note that is said keeps one node as its words change, so the
        // change is announced as the live region's; others are told apart
        // by their words.
        let id: ElementId = match &self.said {
            Some((id, _, _)) => super::child(id, "note"),
            None => SharedString::from(format!("note-{}", self.text)).into(),
        };
        let answer = self.said.map(|(id, count, fresh)| {
            let answers = super::Answerer::of(&id, window, cx);
            let accent = look.accent.hsla();
            if fresh {
                answers.announce(count, super::Answer::Sweep, accent, window, cx);
            } else {
                answers.follow(count, super::Answer::Sweep, accent, window, cx);
            }
            answers.layer(window, cx)
        });
        // One line, cut short in a narrow sidebar: the whole of it, an
        // error's reason included, under the pointer.
        let whole = self.text.clone();
        super::announced(div().id(id), self.text.clone(), self.tone)
            .tooltip(move |window, cx| super::Tooltip::view(whole.clone(), window, cx))
            .relative()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .px(look.space(Space::L))
            .py(look.space(Space::Xs))
            .text_role(TypeRole::Fine, &look)
            .text_color(look.text_color(self.tone))
            .child(self.text)
            .children(answer.flatten())
    }
}

/// A list row: no rule between rows; `hover` at 30 % behind it under the
/// pointer, `selection` when selected with its text in `on_selection`, and
/// the playing row's title and number in the accent. The caller lays out the
/// row's parts with `ListRow::title`, `ListRow::detail` and `ListRow::number`
/// or its own children.
#[derive(IntoElement)]
pub struct ListRow {
    id: ElementId,
    selected: bool,
    playing: bool,
    number: Option<SharedString>,
    title: Option<SharedString>,
    detail: Option<SharedString>,
    trailing: Vec<AnyElement>,
    on_click: Option<Handler<ClickEvent>>,
}

impl ListRow {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            selected: false,
            playing: false,
            number: None,
            title: None,
            detail: None,
            trailing: Vec::new(),
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn playing(mut self, playing: bool) -> Self {
        self.playing = playing;
        self
    }

    /// A track number or place, leading.
    pub fn number(mut self, number: impl Into<SharedString>) -> Self {
        self.number = Some(number.into());
        self
    }

    /// The title, from the library, in its own case.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Secondary text under the title, in `meta`, `muted`.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A figure at the trailing edge, right-aligned in `meta`, `muted`, such
    /// as a duration.
    pub fn figure(mut self, figure: impl Into<SharedString>) -> Self {
        self.trailing
            .push(FigureCell(figure.into()).into_any_element());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

#[derive(IntoElement)]
struct FigureCell(SharedString);

impl RenderOnce for FigureCell {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex_none()
            .text_right()
            .text_role(TypeRole::Meta, &look)
            .child(self.0)
    }
}

impl RenderOnce for ListRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let selected = transition(child(&self.id, "selected"), self.selected, window, cx);
        let on = look.palette.on_selection.hsla();
        let lead = if self.selected {
            on
        } else if self.playing {
            look.text_color(crate::theme::Tone::Accent)
        } else {
            look.palette.ink.hsla()
        };
        let number_color = if self.selected {
            on
        } else if self.playing {
            look.text_color(crate::theme::Tone::Accent)
        } else {
            look.palette.muted.hsla()
        };
        // Detail and figures are muted, which the selection's fill would
        // swallow: on it they take `on_selection` with the title.
        let quiet = if self.selected {
            on
        } else {
            look.palette.muted.hsla()
        };
        let on_click = self.on_click;
        let row = div()
            .id(self.id)
            .relative()
            .flex()
            .items_center()
            .gap(look.space(Space::M))
            .px(look.space(Space::M))
            .py(look.spaced(12.0))
            .min_h(HIT)
            .child(fill(look.palette.selection.hsla(), &selected))
            .children(self.number.map(|number| {
                div()
                    .relative()
                    .flex_none()
                    .min_w(look.spaced(28.0))
                    .text_right()
                    .text_role(TypeRole::Meta, &look)
                    .text_color(number_color)
                    .child(number)
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .children(self.title.map(|title| {
                        div()
                            .text_role(TypeRole::Body, &look)
                            .text_color(lead)
                            .truncate()
                            .child(title)
                    }))
                    .children(self.detail.map(|detail| {
                        div()
                            .text_role(TypeRole::Meta, &look)
                            .text_color(quiet)
                            .truncate()
                            .child(detail)
                    })),
            )
            .children(
                self.trailing
                    .into_iter()
                    .map(|cell| div().relative().text_color(quiet).child(cell)),
            );
        row.when_some(on_click, |row, on_click| {
            row.on_click(move |event, window, cx| on_click(event, window, cx))
        })
    }
}

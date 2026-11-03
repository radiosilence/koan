use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AnyView, App, AppContext, ClickEvent, Context, ElementId, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, px,
};

use super::Handler;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// The theme's popover and menu: a square `bg` panel in a hairline `rule`,
/// with no arrow, no shadow and no material. What it holds is its own; a
/// menu's rows are `MenuRow`s.
#[derive(IntoElement)]
pub struct MenuPanel {
    children: Vec<AnyElement>,
    min_width: Option<f32>,
    /// `min_width` is the panel's width, not only its least.
    fixed: bool,
}

impl MenuPanel {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            min_width: None,
            fixed: false,
        }
    }

    /// At least `points` wide, where given; 180 points otherwise.
    pub fn min_width(mut self, points: Option<f32>) -> Self {
        self.min_width = points;
        self
    }

    /// Exactly its `min_width`, whatever its rows hold: they cut what does
    /// not fit.
    pub fn fixed(mut self, fixed: bool) -> Self {
        self.fixed = fixed;
        self
    }
}

impl Default for MenuPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for MenuPanel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for MenuPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        super::accessible(div().id("menu-panel"), gpui::Role::Menu, None)
            .flex()
            .flex_col()
            .min_w(look.spaced(self.min_width.unwrap_or(180.)))
            .when(self.fixed, |panel| {
                panel.w(look.spaced(self.min_width.unwrap_or(180.)))
            })
            .py(look.space(Space::Xs))
            .bg(look.palette.bg.hsla())
            .border(look.hairline)
            .border_color(look.palette.rule.hsla())
            .children(self.children)
    }
}

/// The theme's popover holding controls rather than a menu's rows (a
/// browser's filters): the menu panel's ground and hairline, `points` wide at
/// 100 %, its content `m` in from the edge and `s` apart.
pub fn popover_panel(points: f32, look: &Look) -> gpui::Div {
    div()
        .w(look.spaced(points))
        .p(look.space(Space::M))
        .flex()
        .flex_col()
        .gap(look.space(Space::S))
        .bg(look.palette.bg.hsla())
        .border(look.hairline)
        .border_color(look.palette.rule.hsla())
        .text_role(TypeRole::Body, look)
        .text_color(look.palette.ink.hsla())
}

/// A row of a menu: its label in `control`, `ink`, after its glyph when
/// icons are on, on `surface` under the pointer or the arrow keys. A choice
/// that is in force is ticked, on a faint accent (`surface` with more
/// contrast); a destructive one is in `bad`.
#[derive(IntoElement)]
pub struct MenuRow {
    id: ElementId,
    label: SharedString,
    icon: Option<Icon>,
    checked: Option<bool>,
    /// Its tick is a setting's, not a choice's (`toggle`).
    toggle: bool,
    highlighted: bool,
    destructive: bool,
    disabled: bool,
    /// A row that opens a submenu: the chevron at its end.
    opens: bool,
    /// Its submenu shows.
    expanded: bool,
    /// The rich row's second line, and whether it warns.
    detail: Option<(SharedString, bool)>,
    /// The rich row: its glyph large in a column of its own, the chosen row
    /// on the choice's ground with its glyph in the accent.
    rich: bool,
    /// A small glyph after the label: how a place is reached.
    reach: Option<Icon>,
    /// Actions at the row's end, before the tick.
    trailing: Vec<MenuAction>,
    /// The columns the rows around it keep at their ends, so every row's
    /// actions line up, glyphs with glyphs and words with words.
    columns: Option<Rc<ActionColumns>>,
    /// The keys that do the same, at the row's end in `muted`.
    keys: Option<SharedString>,
    /// The action the keyboard is on, lit as the pointer lights it.
    keyed: Option<usize>,
    on_click: Option<Handler<ClickEvent>>,
}

/// What a column of a menu's row actions holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Glyph,
    Words,
}

/// The columns at the ends of a menu's rows, last at the end: as many as
/// the fullest row needs, each a glyph's or as wide as the widest words a
/// row puts in it, so actions line up down the menu by kind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActionColumns {
    pub kinds: Vec<Slot>,
    /// A words column's width.
    pub words: Pixels,
}

impl ActionColumns {
    /// The columns for rows whose actions are `rows`, each in order: a run
    /// of kinds every row's actions fit in order, aligned to the end, each
    /// row merged in from its last action, a kind both have kept as one
    /// column.
    pub fn of(rows: &[Vec<Slot>], words: Pixels) -> Self {
        let kinds = rows.iter().fold(Vec::new(), |kinds: Vec<Slot>, row| {
            let (mut i, mut j) = (kinds.len(), row.len());
            let mut merged = Vec::with_capacity(i.max(j));
            while i > 0 || j > 0 {
                if i > 0 && j > 0 && kinds[i - 1] == row[j - 1] {
                    merged.push(kinds[i - 1]);
                    i -= 1;
                    j -= 1;
                } else if i > 0 {
                    merged.push(kinds[i - 1]);
                    i -= 1;
                } else {
                    merged.push(row[j - 1]);
                    j -= 1;
                }
            }
            merged.reverse();
            merged
        });
        Self { kinds, words }
    }

    /// The column each of `row`'s actions stands in: from the last, each in
    /// the nearest column of its kind before the one after it, so a row
    /// with only a glyph has it with the other rows' glyphs, not in a words
    /// column at the end.
    pub fn place(&self, row: &[Slot]) -> Vec<usize> {
        let mut before = self.kinds.len();
        let mut at: Vec<usize> = row
            .iter()
            .rev()
            .map(|kind| {
                let column = (0..before)
                    .rev()
                    .find(|&c| self.kinds[c] == *kind)
                    .unwrap_or(before.saturating_sub(1));
                before = column;
                column
            })
            .collect();
        at.reverse();
        at
    }
}

impl MenuRow {
    pub fn new(id: impl Into<ElementId>, text: impl AsRef<str>) -> Self {
        Self {
            id: id.into(),
            label: label(text),
            icon: None,
            checked: None,
            toggle: false,
            highlighted: false,
            destructive: false,
            disabled: false,
            opens: false,
            expanded: false,
            detail: None,
            rich: false,
            reach: None,
            trailing: Vec::new(),
            columns: None,
            keys: None,
            keyed: None,
            on_click: None,
        }
    }

    /// Keep the menu's action columns at the row's end, its own actions in
    /// those of their kind. Where a column holds words, a choice's tick
    /// stands in it: what is chosen is not moved to, so a row has one or
    /// the other.
    pub fn columns(mut self, columns: Rc<ActionColumns>) -> Self {
        self.columns = Some(columns);
        self
    }

    /// An action of a word or two, quiet as the glyphs are: "move here".
    pub fn text_action(
        self,
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        help: impl Into<SharedString>,
        run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.act(MenuAction {
            id: id.into(),
            face: ActionFace::Text(text.into()),
            help: help.into(),
            lit: false,
            keyed: false,
            run: Arc::new(run),
        })
    }

    /// A quiet, compact glyph at the row's end that does something to what
    /// the row names without choosing it ("forget this device"), in the
    /// accent while `lit`. `help` is its tooltip and its name to a screen
    /// reader.
    pub fn lit_action(
        self,
        id: impl Into<ElementId>,
        icon: Icon,
        help: impl Into<SharedString>,
        lit: bool,
        run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.act(MenuAction {
            id: id.into(),
            face: ActionFace::Glyph(icon),
            help: help.into(),
            lit,
            keyed: false,
            run: Arc::new(run),
        })
    }

    fn act(mut self, action: MenuAction) -> Self {
        self.trailing.push(action);
        self
    }

    /// Lights action `index` of the row, the keyboard's place within it:
    /// what Return runs.
    pub fn keyed_action(mut self, index: Option<usize>) -> Self {
        self.keyed = index;
        self
    }

    /// The keys that do the same, as a menu bar shows them: "⌘K", "Ctrl+K".
    pub fn keys(mut self, keys: impl Into<SharedString>) -> Self {
        self.keys = Some(keys.into());
        self
    }

    /// A row whose label is data rather than the app's words (a preset's
    /// name, a device's), which keeps its case.
    pub fn data(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            label: text.into(),
            ..Self::new(id, "")
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A label that is data rather than the app's words, kept as it is.
    pub fn label_data(mut self, text: impl Into<SharedString>) -> Self {
        self.label = text.into();
        self
    }

    /// One of a set of choices: ticked when in force, with room for the tick
    /// kept when not.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Its tick is a setting's state, not a choice: no chosen ground.
    pub fn toggle(mut self, toggle: bool) -> Self {
        self.toggle = toggle;
        self
    }

    /// Lit by the arrow keys, as the pointer lights it.
    pub fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    /// Not a choice now: `muted`, and nothing under the pointer.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Opens a submenu: a chevron at the row's end.
    pub fn opens(mut self, opens: bool) -> Self {
        self.opens = opens;
        self
    }

    /// The submenu it opens is showing: said to a screen reader.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// A second line under the label, in `fine`, `muted`; in `bad` when it
    /// warns.
    pub fn detail(mut self, detail: impl Into<SharedString>, warns: bool) -> Self {
        self.detail = Some((detail.into(), warns));
        self
    }

    /// The rich row: a place to choose, with its glyph large beside a label
    /// and its detail, the chosen one on the choice's ground and its glyph in
    /// the accent.
    pub fn rich(mut self, rich: bool) -> Self {
        self.rich = rich;
        self
    }

    pub fn reach(mut self, reach: Icon) -> Self {
        self.reach = Some(reach);
        self
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
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

/// The corner of a row's ground, in points.
const ROW_RADIUS: f32 = 4.0;
/// How strongly the accent grounds the chosen row of a menu of choices.
const PICKED: f32 = 0.12;

/// How wide a rich row's second line may be before it ends in an ellipsis:
/// the Mac picker's width less the glyph column and the row's insets.
const DETAIL_WIDTH: f32 = 260.0;

/// A row's secondary action: a glyph in `muted`, its slot's width, `surface`
/// under the pointer, with its tooltip.
#[derive(IntoElement)]
pub(crate) struct MenuAction {
    id: ElementId,
    face: ActionFace,
    help: SharedString,
    lit: bool,
    /// The keyboard is on it: lit as under the pointer.
    keyed: bool,
    run: super::Handler<ClickEvent>,
}

/// What an action shows: a glyph, or a word or two where a glyph would not
/// say it ("move here").
pub enum ActionFace {
    Glyph(Icon),
    Text(SharedString),
}

impl MenuAction {
    /// A glyph column's width, in points.
    const SLOT: f32 = 28.0;

    /// The width a words column needs for `words`, measured in the face
    /// and size they are drawn in, with their padding.
    pub(crate) fn words_width(words: &[SharedString], window: &Window, cx: &App) -> Pixels {
        let look = Look::of(window, cx);
        let widest = super::text_widths(words, TypeRole::Meta, window, cx)
            .into_iter()
            .fold(px(0.0), Pixels::max);
        widest.ceil() + look.space(Space::S) * 2.0
    }
}

impl RenderOnce for MenuAction {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let (help, run) = (self.help.clone(), self.run);
        // Words shown lead the name a screen reader gives it, so the name
        // holds what is seen ("move here") as well as what it does.
        let name: SharedString = match &self.face {
            ActionFace::Text(text) => format!("{text}: {}", self.help).into(),
            ActionFace::Glyph(_) => self.help.clone(),
        };
        super::accessible(div().id(self.id), gpui::Role::Button, Some(name))
            .flex()
            .items_center()
            .justify_center()
            .h(look.spaced(MenuAction::SLOT))
            .cursor_pointer()
            .when(self.keyed, |button| button.bg(look.palette.hover.hsla()))
            .hover(|button| button.bg(look.palette.hover.hsla()))
            .map(|button| {
                let color = if self.lit {
                    look.color(Tone::Accent)
                } else {
                    look.palette.muted.hsla()
                };
                match self.face {
                    ActionFace::Glyph(icon) => button
                        .w(look.spaced(MenuAction::SLOT))
                        .child(KoanIcon::new(icon).role(TypeRole::Control).color(color)),
                    ActionFace::Text(text) => button
                        .w_full()
                        .px(look.space(Space::S))
                        .text_role(TypeRole::Meta, &look)
                        .text_color(color)
                        .whitespace_nowrap()
                        .child(text),
                }
            })
            .tooltip(move |window, cx| super::Tooltip::view(help.clone(), window, cx))
            // A screen reader's press, as a pointer's.
            .on_a11y_action(gpui::AccessibleAction::Click, {
                let run = run.clone();
                move |_, window, cx| run(&gpui::ClickEvent::default(), window, cx)
            })
            .on_click(move |event, window, cx| {
                cx.stop_propagation();
                run(event, window, cx)
            })
    }
}

impl RenderOnce for MenuRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let color = look.color(if self.disabled {
            Tone::Muted
        } else if self.destructive {
            Tone::Bad
        } else {
            Tone::Ink
        });
        let surface = look.palette.surface.hsla();
        let tick = look.points(TypeRole::Control);
        let on_click = self.on_click;
        let chosen = self.rich && self.checked == Some(true);
        let picked = self.checked == Some(true);
        // The chosen place's ground: a choice in force, not a setting's tick.
        let grounded = picked && !self.toggle;
        let muted = look.palette.muted.hsla();
        let accent = look.color(Tone::Accent);
        let (label, detail, reach) = (self.label, self.detail, self.reach);
        let spoken = label.clone();
        let described = detail.as_ref().map(|(d, _)| d.clone());
        // A name too long for a fixed panel is cut, not let widen it.
        let name = div()
            .flex()
            .items_center()
            .min_w_0()
            .gap(look.space(Space::Xs))
            .child(div().min_w_0().truncate().child(label))
            .children(reach.map(|icon| KoanIcon::new(icon).role(TypeRole::Fine).color(muted)));
        // One line of detail, as wide as `DETAIL_WIDTH`; what does not fit
        // is in its tooltip.
        let text = match detail {
            Some((detail, warns)) => {
                // Its whole text as a tooltip only where the line is cut.
                let cut =
                    super::text_widths(std::slice::from_ref(&detail), TypeRole::Fine, window, cx)
                        .first()
                        .is_some_and(|w| *w > look.spaced(DETAIL_WIDTH));
                let full = detail.clone();
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(name)
                    .child(
                        div()
                            .id("detail")
                            .max_w(look.spaced(DETAIL_WIDTH))
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_role(TypeRole::Fine, &look)
                            .text_color(if warns {
                                look.palette.bad.hsla()
                            } else {
                                muted
                            })
                            .child(detail)
                            .when(cut, |line| {
                                line.tooltip(move |window, cx| {
                                    super::Tooltip::view(full.clone(), window, cx)
                                })
                            }),
                    )
            }
            None => div().relative().flex_1().min_w_0().child(name),
        };
        let rich = self.rich;
        // The row's actions, each in the menu's column of its kind, then its
        // tick when it is the choice. The tick stands in a words column
        // where there is one ("move here"): what is chosen is never moved
        // to, so the two never meet. Beside glyphs every row may carry, it
        // keeps its own column.
        let columns = self.columns.clone().unwrap_or_default();
        let shares = columns.kinds.contains(&Slot::Words);
        let keyed = self.keyed;
        // A disabled row with actions is still reached by the arrows, and
        // lit where they are.
        let reached = !self.trailing.is_empty();
        let own_words: Vec<SharedString> = self
            .trailing
            .iter()
            .filter_map(|action| match &action.face {
                ActionFace::Text(text) => Some(text.clone()),
                ActionFace::Glyph(_) => None,
            })
            .collect();
        let mut filled: Vec<(Slot, Option<AnyElement>)> = self
            .trailing
            .into_iter()
            .enumerate()
            .map(|(i, mut action)| {
                action.keyed = keyed == Some(i);
                let kind = match action.face {
                    ActionFace::Glyph(_) => Slot::Glyph,
                    ActionFace::Text(_) => Slot::Words,
                };
                (kind, Some(action.into_any_element()))
            })
            .collect();
        // A disabled choice still says it is the choice, quietly: muted, as
        // its label is, never the accent of something to choose.
        let ticked = if self.disabled {
            look.palette.muted.hsla()
        } else {
            accent
        };
        if shares && self.checked == Some(true) {
            filled.push((
                Slot::Words,
                Some(
                    KoanIcon::new(Icon::Check)
                        .points(tick)
                        .color(ticked)
                        .into_any_element(),
                ),
            ));
        }
        // A row out of any menu's columns lays its actions out as they come.
        let columns = if columns.kinds.len() < filled.len() {
            Rc::new(ActionColumns::of(
                &[filled.iter().map(|(kind, _)| *kind).collect()],
                MenuAction::words_width(&own_words, window, cx),
            ))
        } else {
            columns
        };
        let placed = columns.place(&filled.iter().map(|(kind, _)| *kind).collect::<Vec<_>>());
        let mut cells: Vec<Option<AnyElement>> = (0..columns.kinds.len()).map(|_| None).collect();
        for (column, (_, element)) in placed.into_iter().zip(filled) {
            if cells[column].is_none() {
                cells[column] = element;
            }
        }
        // A menu item to a screen reader, a checkbox item where it ticks,
        // with its second line as its description.
        let role = if self.checked.is_some() {
            gpui::Role::MenuItemCheckBox
        } else {
            gpui::Role::MenuItem
        };
        let row = super::accessible(div().id(self.id), role, Some(spoken))
            .aria_disabled(self.disabled)
            .when_some(self.checked, |row, checked| {
                row.aria_toggled(super::toggled(checked))
            })
            .when_some(described, |row, d| row.aria_description(d))
            .when(self.opens, |row| row.aria_expanded(self.expanded))
            .relative()
            .flex()
            .items_center()
            .gap(look.space(if rich { Space::M } else { Space::S }))
            // Inset from the panel's edges, its ground rounded, as the Mac's
            // picker draws a chosen row; the content keeps the panel's one
            // left edge.
            .mx(look.space(Space::Xs))
            .px(look.space(Space::M) - look.space(Space::Xs))
            .py(look.space(if rich { Space::S } else { Space::Xs }))
            .rounded(look.spaced(ROW_RADIUS))
            // A rich row keeps two lines' height with one line of text, its
            // title centred, so a list of them does not jump as details come
            // and go.
            .when(rich, |row| {
                row.min_h(
                    look.line_height(TypeRole::Body)
                        + look.line_height(TypeRole::Fine)
                        + look.space(Space::S) * 2.0,
                )
            })
            .text_role(
                if rich {
                    TypeRole::Body
                } else {
                    TypeRole::Control
                },
                &look,
            )
            .text_color(color)
            .whitespace_nowrap()
            // The choice on the accent, faint, in every menu of choices, a
            // disabled one too (an EQ preset edited), which only does not
            // light; a setting's tick takes none. The arrow keys' highlight
            // on the choice is the pointer's, the stronger tint. With more
            // contrast asked for, a faint tint could vanish: the choice takes
            // `surface`, opaque. The highlight on `surface` elsewhere.
            .when(grounded, |row| {
                let (rest, lit) = if look.contrast {
                    (surface, look.palette.hover.hsla())
                } else {
                    (accent.opacity(PICKED), accent.opacity(PICKED * 1.5))
                };
                let live = !self.disabled;
                row.bg(if self.highlighted && live { lit } else { rest })
                    .when(live, |row| row.hover(move |row| row.bg(lit)))
            })
            .when(
                !grounded && self.highlighted && (!self.disabled || reached),
                |row| row.bg(surface),
            )
            .when(!grounded && !self.disabled, |row| {
                row.hover(move |row| row.bg(surface))
            })
            .children(self.icon.filter(|_| look.icons || rich).map(|icon| {
                if rich {
                    // The place's glyph, at the size of a small title, in a
                    // column so the names line up.
                    div()
                        .relative()
                        .flex_none()
                        .w(look.spaced(28.0))
                        .flex()
                        .justify_center()
                        .child(
                            KoanIcon::new(icon)
                                .role(TypeRole::TitleSmall)
                                .color(if chosen { accent } else { muted }),
                        )
                } else {
                    div()
                        .relative()
                        .child(KoanIcon::new(icon).role(TypeRole::Control).color(color))
                }
            }))
            .child(text)
            // Every column of the menu, empty ones keeping their room, so a
            // row's actions and its tick sit level with every other row's.
            .children(columns.kinds.iter().zip(cells).map(|(kind, content)| {
                div()
                    .relative()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .w(match kind {
                        Slot::Glyph => look.spaced(MenuAction::SLOT),
                        Slot::Words => columns.words,
                    })
                    .children(content)
            }))
            .children(self.keys.map(|keys| {
                div()
                    .relative()
                    .flex_none()
                    .ml(look.space(Space::L))
                    .text_color(muted)
                    .child(keys)
            }))
            // The tick at the row's end, after `m`, as the Mac's menus put
            // it; its room is kept on every row of a set of choices. In a
            // menu with slots it is in the last of them instead.
            .children(self.checked.filter(|_| !shares).map(|checked| {
                div()
                    .relative()
                    .flex_none()
                    .ml(look.space(Space::M))
                    .size(gpui::px(crate::theme::icons::side(tick)))
                    .when(checked, |cell| {
                        cell.child(KoanIcon::new(Icon::Check).points(tick).color(ticked))
                    })
            }))
            .when(self.opens, |row| {
                row.child(
                    div()
                        .relative()
                        .flex_none()
                        .ml(look.space(Space::M))
                        .child(KoanIcon::new(Icon::Disclosure).points(tick).color(color)),
                )
            });
        let on_click = on_click.filter(|_| !self.disabled);
        row.when_some(on_click, |row, on_click| {
            super::answers_click(row, on_click.clone())
                .cursor_pointer()
                .on_click(move |event, window, cx| on_click(event, window, cx))
        })
    }
}

/// The rule between a menu's groups: a hairline of `rule` across the panel,
/// `xs` above and below.
#[derive(IntoElement)]
pub struct MenuSeparator;

impl RenderOnce for MenuSeparator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .h(look.hairline)
            .my(look.space(Space::Xs))
            .bg(look.palette.rule.hsla())
    }
}

/// A tooltip, in the theme's panel: `fine`, `ink`, on `bg` in a hairline
/// `rule`.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }

    /// The view an element's `.tooltip(..)` builder returns.
    pub fn view(text: SharedString, _window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|_| Tooltip::new(text)).into()
    }
}

/// How many tooltips have been drawn: what a test hovering a control reads
/// to know its tooltip showed.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub static TOOLTIPS_DRAWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(debug_assertions)]
        TOOLTIPS_DRAWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let look = Look::of(window, cx);
        div()
            .px(look.space(Space::S))
            .py(look.space(Space::Xs))
            .bg(look.palette.bg.hsla())
            .border(look.hairline)
            .border_color(look.palette.rule.hsla())
            .text_role(TypeRole::Fine, &look)
            .text_color(look.palette.ink.hsla())
            .child(self.text.clone())
    }
}

/// A panel standing by the control that opened it (a row's "more" menu),
/// over the page and kept inside the window; a press anywhere else calls
/// `on_dismiss`.
pub fn anchored_panel(
    id: impl Into<ElementId>,
    content: impl IntoElement,
    on_dismiss: impl Fn(&mut App) + 'static,
    look: &Look,
) -> impl IntoElement {
    deferred(
        anchored()
            .snap_to_window_with_margin(look.space(Space::S))
            .child(
                div()
                    .id(id)
                    .occlude()
                    .on_mouse_down_out(move |_, _, cx| on_dismiss(cx))
                    .child(content),
            ),
    )
    .with_priority(1)
}

#[cfg(test)]
mod action_column_tests {
    use super::*;
    use core::prelude::v1::test;

    /// A row of a glyph alone keeps it with the other rows' glyphs, not in
    /// the words column at the end; a tick stands in the words column.
    #[test]
    fn actions_line_up_by_kind() {
        use Slot::{Glyph, Words};
        let columns = ActionColumns::of(&[vec![Glyph, Words], vec![Glyph], vec![Words]], px(80.));
        assert_eq!(columns.kinds, [Glyph, Words]);
        assert_eq!(columns.place(&[Glyph, Words]), [0, 1]);
        assert_eq!(columns.place(&[Glyph]), [0], "with the glyphs");
        assert_eq!(columns.place(&[Words]), [1], "a tick with the words");
        let glyphs = ActionColumns::of(&[vec![Glyph, Glyph], vec![Glyph]], px(0.));
        assert_eq!(glyphs.place(&[Glyph]), [1], "the last of the glyphs");
        // Two glyphs beside a glyph and words: a column for each, none lost.
        let both = ActionColumns::of(&[vec![Glyph, Words], vec![Glyph, Glyph]], px(80.));
        assert_eq!(both.kinds, [Glyph, Glyph, Words]);
        assert_eq!(both.place(&[Glyph, Glyph]), [0, 1]);
        assert_eq!(both.place(&[Glyph, Words]), [1, 2]);
    }
}

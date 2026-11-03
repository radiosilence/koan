//! The theme's menu: the panel and its rows as a view of its own, opened as
//! a context menu at the pointer or below a button. Up and Down move through
//! it, Right and Return open a submenu, Return chooses, Escape and a click
//! outside close it. gpui-component's `PopupMenu` draws its own padding,
//! corners and type, which no theme setting reaches; this is the Mac's
//! `KoanMenu` instead.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{MenuPanel, MenuRow, MenuSeparator};
use crate::theme::{Icon, KoanStyled, Look, Space};

type Run = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type Build = Rc<dyn Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu>;
/// A button at a rich row's end, made each time the menu draws.
type Trailing = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// An entry of a menu, built as `PopupMenuItem` is.
pub struct MenuEntry {
    label: SharedString,
    data: bool,
    icon: Option<Icon>,
    checked: Option<bool>,
    destructive: bool,
    disabled: bool,
    detail: Option<(SharedString, bool)>,
    rich: bool,
    reach: Option<Icon>,
    trailing: Vec<Trailing>,
    keys: Option<SharedString>,
    run: Option<Run>,
    stays_open: bool,
    actions: Vec<(super::panel::ActionFace, SharedString, bool, Run)>,
}

impl MenuEntry {
    /// An entry in the app's words, drawn lowercase.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            label: text.into(),
            data: false,
            icon: None,
            checked: None,
            destructive: false,
            disabled: false,
            detail: None,
            rich: false,
            reach: None,
            trailing: Vec::new(),
            keys: None,
            run: None,
            stays_open: false,
            actions: Vec::new(),
        }
    }

    /// A quiet glyph at the row's end that acts on what the row names
    /// without choosing it ("move here", "forget this device"), with `help`
    /// as its tooltip. Every row of the menu keeps the same slots, so the
    /// actions line up.
    pub fn action(
        mut self,
        icon: Icon,
        help: impl Into<SharedString>,
        run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.actions.push((
            super::panel::ActionFace::Glyph(icon),
            help.into(),
            false,
            Rc::new(run),
        ));
        self
    }

    /// An action of a word or two, quiet as the glyphs are: "move here". As
    /// a row's last action it takes the last slot, which every row then
    /// keeps as wide.
    pub fn text_action(
        mut self,
        text: impl Into<SharedString>,
        help: impl Into<SharedString>,
        run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.actions.push((
            super::panel::ActionFace::Text(text.into()),
            help.into(),
            false,
            Rc::new(run),
        ));
        self
    }

    /// `action`, its glyph in the accent while `lit`: a setting that is on
    /// for what the row names, as an output's EQ.
    pub fn lit_action(
        mut self,
        icon: Icon,
        help: impl Into<SharedString>,
        lit: bool,
        run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.actions.push((
            super::panel::ActionFace::Glyph(icon),
            help.into(),
            lit,
            Rc::new(run),
        ));
        self
    }

    /// Choosing it leaves the menu open: a choice among outputs or devices,
    /// whose tick and details the menu shows moving as it is refilled.
    pub fn stays_open(mut self) -> Self {
        self.stays_open = true;
        self
    }

    /// The keys that do the same, at the row's end in `muted`: a menu drawn
    /// in the window where there is no menu bar.
    pub fn keys(mut self, keys: impl Into<SharedString>) -> Self {
        self.keys = Some(keys.into());
        self
    }

    /// An entry that is data (a playlist, a device), which keeps its case.
    pub fn data(text: impl Into<SharedString>) -> Self {
        Self {
            data: true,
            ..Self::new(text)
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// One of a set of choices, ticked when in force.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// A place to choose, drawn as the Mac's output picker draws one: its
    /// glyph large, the label over `detail`, the chosen one on `surface`.
    pub fn rich(mut self, detail: impl Into<SharedString>) -> Self {
        self.rich = true;
        self.detail = Some((detail.into(), false));
        self
    }

    /// The rich row's detail is a warning, in `bad`.
    pub fn warns(mut self, warns: bool) -> Self {
        if let Some((_, w)) = self.detail.as_mut() {
            *w = warns;
        }
        self
    }

    /// A small glyph after the label: how the place is reached.
    pub fn reach(mut self, reach: Icon) -> Self {
        self.reach = Some(reach);
        self
    }

    /// A button at the row's end.
    pub fn trailing(
        mut self,
        make: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.trailing.push(Rc::new(make));
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.run = Some(Rc::new(run));
        self
    }
}

enum Entry {
    Item(MenuEntry),
    Separator,
    /// A section's name over its entries, chosen by nothing.
    Heading(SharedString),
    /// A control of its own in the row's insets, such as a slider.
    Control(Trailing),
    /// A sentence explaining the entries around it, wrapped.
    Note(SharedString),
    Submenu {
        label: SharedString,
        icon: Option<Icon>,
        build: Build,
        open: Option<Entity<Menu>>,
    },
}

/// How wide a note's lines run, in points before the text size scales them:
/// the Mac's output picker's width.
const NOTE_WIDTH: f32 = 340.;

pub struct Menu {
    entries: Vec<Entry>,
    highlighted: Option<usize>,
    focus: FocusHandle,
    /// Where it was drawn last, for the menu it opened from to tell a press
    /// in it from one outside them both.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The menus it opened from, as a submenu, nearest first: a press in
    /// any of them is not outside.
    ancestors: Vec<Rc<Cell<Option<Bounds<Pixels>>>>>,
    /// The least the panel is wide, in points, so a menu whose rows change
    /// while it is open does not change size with them.
    min_width: Option<f32>,
    _submenu: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for Menu {}

/// A submenu asked to close itself alone, by Left or Escape: the menu it
/// opened from stays, with the keyboard back on it.
pub struct Back;

impl EventEmitter<Back> for Menu {}

impl Focusable for Menu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Menu {
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        build: impl FnOnce(Menu, &mut Window, &mut Context<Menu>) -> Menu,
    ) -> Entity<Menu> {
        cx.new(|cx| {
            let menu = Menu {
                entries: Vec::new(),
                highlighted: None,
                focus: cx.focus_handle(),
                bounds: Rc::default(),
                ancestors: Vec::new(),
                min_width: None,
                _submenu: Vec::new(),
            };
            build(menu, window, cx)
        })
    }

    pub fn item(mut self, entry: MenuEntry) -> Self {
        self.entries.push(Entry::Item(entry));
        self
    }

    /// A line of the menu that says something and does nothing.
    pub fn label(self, text: impl Into<SharedString>) -> Self {
        self.item(MenuEntry::new(text).disabled(true))
    }

    /// A control in a row of its own, in the rows' insets: a renderer's
    /// volume. Made each time the menu draws, chosen by nothing; it keeps its
    /// own focus and keys.
    pub fn control(mut self, make: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        self.entries.push(Entry::Control(Rc::new(make)));
        self
    }

    /// A sentence about the entries around it, in `fine`, `muted`, wrapped
    /// at the menu's reading width rather than widening it.
    pub fn note(mut self, text: impl Into<SharedString>) -> Self {
        self.entries.push(Entry::Note(text.into()));
        self
    }

    /// Makes the entries again while the menu is open, for a menu whose
    /// content arrives after it opens (renderers found on the network). The
    /// highlight stays on its row while that row is still there.
    pub fn refill(
        &mut self,
        cx: &mut Context<Menu>,
        build: impl FnOnce(Menu, &mut Context<Menu>) -> Menu,
    ) {
        let empty = Menu {
            entries: Vec::new(),
            highlighted: None,
            focus: self.focus.clone(),
            bounds: self.bounds.clone(),
            ancestors: self.ancestors.clone(),
            min_width: self.min_width,
            _submenu: Vec::new(),
        };
        let built = build(empty, cx);
        self.entries = built.entries;
        self.min_width = built.min_width;
        self._submenu.clear();
        self.highlighted = self
            .highlighted
            .filter(|&i| i < self.entries.len() && self.choosable(i));
        cx.notify();
    }

    /// The width a picker of places (outputs, devices) keeps whatever its
    /// rows say: a rich row's glyph, its name over one line of detail, and
    /// its actions' slots, a "move here" among them.
    pub const PICKER_WIDTH: f32 = 440.0;

    /// Keep the panel at least `points` wide, so it holds its size as rows
    /// come, go and change while it is open.
    pub fn min_width(mut self, points: f32) -> Self {
        self.min_width = Some(points);
        self
    }

    /// A rule between groups; one at the start, at the end or after another
    /// is left out.
    pub fn separator(mut self) -> Self {
        if !matches!(self.entries.last(), None | Some(Entry::Separator)) {
            self.entries.push(Entry::Separator);
        }
        self
    }

    /// A section: a rule, then its name in `fine`, `muted`, over the
    /// entries that follow.
    pub fn section(self, title: impl AsRef<str>) -> Self {
        let mut menu = self.separator();
        menu.entries
            .push(Entry::Heading(crate::theme::label(title)));
        menu
    }

    pub fn submenu(
        self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Menu>,
        build: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.submenu_inner(text.into(), None, window, cx, build)
    }

    /// A submenu with the glyph the Mac's menu gives it.
    pub fn submenu_icon(
        self,
        text: impl Into<SharedString>,
        icon: Icon,
        window: &mut Window,
        cx: &mut Context<Menu>,
        build: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.submenu_inner(text.into(), Some(icon), window, cx, build)
    }

    fn submenu_inner(
        mut self,
        label: SharedString,
        icon: Option<Icon>,
        _: &mut Window,
        _: &mut Context<Menu>,
        build: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.entries.push(Entry::Submenu {
            label,
            icon,
            build: Rc::new(build),
            open: None,
        });
        self
    }

    fn choosable(&self, index: usize) -> bool {
        match &self.entries[index] {
            Entry::Item(item) => !item.disabled && item.run.is_some(),
            Entry::Submenu { .. } => true,
            Entry::Separator | Entry::Heading(_) | Entry::Control(_) | Entry::Note(_) => false,
        }
    }

    /// The next entry that can be chosen, from `from` in `step`'s direction,
    /// wrapping.
    fn step(&self, step: isize) -> Option<usize> {
        let count = self.entries.len() as isize;
        let start = self
            .highlighted
            .map_or(if step > 0 { -1 } else { count }, |i| i as isize);
        (1..=count)
            .map(|n| (start + step * n).rem_euclid(count) as usize)
            .find(|&i| self.choosable(i))
    }

    fn highlight(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if self.highlighted == index {
            return;
        }
        self.highlighted = index;
        // Another row under the pointer or the keys closes an open submenu.
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if let Entry::Submenu { open, .. } = entry
                && Some(i) != index
            {
                *open = None;
            }
        }
        self._submenu.clear();
        cx.notify();
    }

    fn open_submenu(
        &mut self,
        index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Entry::Submenu { build, open, .. }) = self.entries.get(index) else {
            return;
        };
        if open.is_none() {
            let build = build.clone();
            let child = Menu::build(window, cx, move |menu, window, cx| build(menu, window, cx));
            let mut ancestors = vec![self.bounds.clone()];
            ancestors.extend(self.ancestors.iter().cloned());
            child.update(cx, |child, _| child.ancestors = ancestors);
            self._submenu = vec![
                // Choosing in the submenu closes this menu too.
                cx.subscribe(&child, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent)),
                // Stepping back out of it closes it alone.
                cx.subscribe_in(&child, window, move |menu, _, _: &Back, window, cx| {
                    if let Some(Entry::Submenu { open, .. }) = menu.entries.get_mut(index) {
                        *open = None;
                    }
                    menu._submenu.clear();
                    window.focus(&menu.focus, cx);
                    cx.notify();
                }),
            ];
            let Some(Entry::Submenu { open, .. }) = self.entries.get_mut(index) else {
                return;
            };
            *open = Some(child);
        }
        if focus
            && let Some(Entry::Submenu {
                open: Some(child), ..
            }) = self.entries.get(index)
        {
            let child = child.clone();
            child.update(cx, |child, cx| {
                child.highlighted = child.step(1);
                cx.notify();
            });
            window.focus(&child.focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// Whether a submenu open from this one, or one open from that, was drawn
    /// where `at` is.
    fn submenus_hold(&self, at: Point<Pixels>, cx: &App) -> bool {
        self.entries.iter().any(|entry| match entry {
            Entry::Submenu {
                open: Some(child), ..
            } => {
                let child = child.read(cx);
                child.bounds.get().is_some_and(|b| b.contains(&at)) || child.submenus_hold(at, cx)
            }
            _ => false,
        })
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        match &self.entries[index] {
            Entry::Item(item) if !item.disabled => {
                if let Some(run) = item.run.clone() {
                    if !item.stays_open {
                        cx.emit(DismissEvent);
                    }
                    run(&ClickEvent::default(), window, cx);
                }
            }
            Entry::Submenu { .. } => self.open_submenu(index, true, window, cx),
            _ => {}
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "down" => {
                let next = self.step(1);
                self.highlight(next, cx);
            }
            "up" => {
                let next = self.step(-1);
                self.highlight(next, cx);
            }
            "enter" | "space" | "right" => {
                if let Some(index) = self.highlighted {
                    if event.keystroke.key == "right"
                        && !matches!(self.entries[index], Entry::Submenu { .. })
                    {
                        return;
                    }
                    self.choose(index, window, cx);
                }
            }
            // A submenu steps back to the menu it opened from; the menu
            // itself closes.
            "escape" | "left" if !self.ancestors.is_empty() => cx.emit(Back),
            "escape" => cx.emit(DismissEvent),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let menu = cx.entity();
        let mut panel = MenuPanel::new().min_width(self.min_width);
        // Every row keeps as many slots as the fullest needs; the last is
        // wide when any row's last action is words, and a choice's tick then
        // counts as that slot's.
        let filled = |item: &MenuEntry| item.trailing.len() + item.actions.len();
        let last_wide = self.entries.iter().any(|entry| match entry {
            Entry::Item(item) => matches!(
                item.actions.last(),
                Some((super::panel::ActionFace::Text(_), ..))
            ),
            _ => false,
        });
        let slots = self
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Item(item) => {
                    filled(item) + usize::from(last_wide && item.checked == Some(true))
                }
                _ => 0,
            })
            .max()
            .unwrap_or(0);
        for (index, entry) in self.entries.iter().enumerate() {
            let highlighted = self.highlighted == Some(index);
            let hovered = {
                let menu = menu.clone();
                move |hovered: &bool, window: &mut Window, cx: &mut App| {
                    if *hovered {
                        menu.update(cx, |menu, cx| {
                            menu.highlight(Some(index), cx);
                            if matches!(menu.entries[index], Entry::Submenu { .. }) {
                                menu.open_submenu(index, false, window, cx);
                            }
                        })
                    }
                }
            };
            panel = match entry {
                Entry::Separator => panel.child(MenuSeparator),
                Entry::Heading(title) => panel.child(
                    div()
                        .px(look.space(Space::M))
                        .pt(look.space(Space::Xs))
                        .text_role(crate::theme::TypeRole::Fine, &look)
                        .text_color(look.palette.muted.hsla())
                        .child(title.clone()),
                ),
                Entry::Control(make) => panel.child(
                    div()
                        .px(look.space(Space::M))
                        .py(look.space(Space::Xs))
                        .child(make(window, cx)),
                ),
                Entry::Note(text) => panel.child(
                    div()
                        .px(look.space(Space::M))
                        .py(look.space(Space::Xs))
                        .max_w(look.spaced(NOTE_WIDTH))
                        .text_role(crate::theme::TypeRole::Fine, &look)
                        .text_color(look.palette.muted.hsla())
                        .child(text.clone()),
                ),
                Entry::Item(item) => {
                    let mut row = if item.data {
                        MenuRow::data(("entry", index), item.label.clone())
                    } else {
                        MenuRow::new(("entry", index), &item.label)
                    }
                    .highlighted(highlighted)
                    .destructive(item.destructive)
                    .disabled(item.disabled || item.run.is_none());
                    if let Some(checked) = item.checked {
                        row = row.checked(checked);
                    }
                    if let Some(icon) = item.icon {
                        row = row.icon(icon);
                    }
                    if let Some((detail, warns)) = item.detail.clone() {
                        row = row.detail(detail, warns);
                    }
                    if let Some(reach) = item.reach {
                        row = row.reach(reach);
                    }
                    row = row.rich(item.rich);
                    for make in &item.trailing {
                        row = row.trailing(make(window, cx));
                    }
                    for (k, (face, help, lit, run)) in item.actions.iter().enumerate() {
                        let run = run.clone();
                        let id = ("action", index * 8 + k);
                        let act = move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                            run(event, window, cx)
                        };
                        row = match face {
                            super::panel::ActionFace::Glyph(icon) => {
                                row.lit_action(id, *icon, help.clone(), *lit, act)
                            }
                            super::panel::ActionFace::Text(text) => {
                                row.text_action(id, text.clone(), help.clone(), act)
                            }
                        };
                    }
                    row = row.slots(slots, last_wide);
                    if let Some(keys) = item.keys.clone() {
                        row = row.keys(keys);
                    }
                    let menu = menu.clone();
                    row = row.on_click(move |_, window, cx| {
                        menu.update(cx, |menu, cx| menu.choose(index, window, cx))
                    });
                    let label = item.label.clone();
                    panel.child(
                        div()
                            .id(("hover", index))
                            .debug_selector(move || format!("menu: {label}"))
                            .on_hover(hovered)
                            .child(row),
                    )
                }
                Entry::Submenu {
                    label, icon, open, ..
                } => {
                    let mut row = MenuRow::new(("entry", index), label)
                        .highlighted(highlighted)
                        .opens(true)
                        .expanded(open.is_some());
                    if let Some(icon) = icon {
                        row = row.icon(*icon);
                    }
                    let name = label.clone();
                    panel.child(
                        div()
                            .id(("hover", index))
                            .debug_selector(move || format!("menu: {name}"))
                            .relative()
                            .on_hover(hovered)
                            .child(row)
                            .children(open.clone().map(|child| {
                                // Beside the row, its first row level with it.
                                div()
                                    .absolute()
                                    .left_full()
                                    .top(-look.space(Space::Xs))
                                    .child(child)
                            })),
                    )
                }
            };
        }
        let drawn = self.bounds.clone();
        div()
            .relative()
            .track_focus(&self.focus)
            .key_context("KoanMenu")
            .on_key_down(cx.listener(Self::key))
            // A press in an open submenu, drawn outside this menu, or in the
            // menu this one opened from, is not a press outside.
            .on_mouse_down_out(cx.listener(|menu, event: &MouseDownEvent, _, cx| {
                let at = event.position;
                let in_ancestor = menu
                    .ancestors
                    .iter()
                    .any(|cell| cell.get().is_some_and(|b| b.contains(&at)));
                if in_ancestor || menu.submenus_hold(at, cx) {
                    return;
                }
                cx.emit(DismissEvent)
            }))
            .occlude()
            .child(panel)
            .child(
                // Over the panel, from its corner: without an inset an
                // absolute child sits where the flow would put it, under the
                // panel, and the bounds it records miss the menu.
                canvas(move |bounds, _, _| drawn.set(Some(bounds)), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            // Tab stays in the open menu rather than reaching the page under it.
            .focus_trap("menu-trap", &self.focus)
    }
}

/// What a context menu holds while it is open: where it opened, the menu,
/// and the subscription that closes it.
#[derive(Default)]
struct Open(Option<(Point<Pixels>, Entity<Menu>, Subscription)>);

/// A context menu on any element: `build` makes the menu each time the
/// secondary button is pressed on it, and the menu opens at the pointer.
pub trait KoanContextMenu: InteractiveElement + ParentElement + Sized {
    fn koan_context_menu(
        self,
        id: impl Into<ElementId>,
        window: &mut Window,
        cx: &mut App,
        build: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.koan_context_menu_at(id, None, window, cx, build)
    }

    /// `koan_context_menu`, which the menu key also opens, under the
    /// element's keyboard cursor (`spot`) where one is laid out, and at the
    /// pointer otherwise.
    fn koan_context_menu_at(
        self,
        id: impl Into<ElementId>,
        spot: Option<super::CursorSpot>,
        window: &mut Window,
        cx: &mut App,
        build: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        let state = window.use_keyed_state(id.into(), cx, |_, _| Open::default());
        let shown = state
            .read(cx)
            .0
            .as_ref()
            .map(|(at, menu, _)| (*at, menu.clone()));
        let build = Rc::new(build);
        let opener = state.clone();
        let open = Rc::new(
            move |at: Point<Pixels>, window: &mut Window, cx: &mut App| {
                let build = build.clone();
                let menu = Menu::build(window, cx, move |menu, window, cx| build(menu, window, cx));
                // Nothing to act on, nothing to open.
                if menu.read(cx).entries.is_empty() {
                    return;
                }
                // Where the keyboard was, given back as the menu closes: a list's
                // keys work again after a choice or Escape.
                let before = window.focused(cx);
                let own = menu.focus_handle(cx);
                opener.update(cx, |open, cx| {
                    let closing = cx.subscribe_in(
                        &menu,
                        window,
                        move |open: &mut Open, _, _: &DismissEvent, window, cx| {
                            open.0 = None;
                            // Only from the menu: a choice that put up a sheet
                            // or opened a field keeps the keyboard where it went.
                            let in_menu =
                                window.focused(cx).is_none() || own.contains_focused(window, cx);
                            if in_menu && let Some(before) = &before {
                                window.focus(before, cx);
                            }
                            cx.notify();
                        },
                    );
                    open.0 = Some((at, menu.clone(), closing));
                    cx.notify();
                });
                window.focus(&menu.focus_handle(cx), cx);
            },
        );
        let by_key = open.clone();
        let mut this = self
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                open(event.position, window, cx)
            })
            // The menu key: under the keyboard's cursor, where its ring is
            // laid out in the frames after the key. Bounds from before it are
            // not trusted: the list may have scrolled the row away since, or
            // just revealed it. The first callback runs before the next frame
            // draws, the second after. At the pointer when there is no cursor
            // or no ring was drawn.
            .on_action(move |_: &super::list_keys::ShowMenu, window, cx| {
                let Some(spot) = spot.clone() else {
                    return by_key(window.mouse_position(), window, cx);
                };
                spot.forget_bounds();
                window.refresh();
                let by_key = by_key.clone();
                window.on_next_frame(move |window, _| {
                    window.refresh();
                    window.on_next_frame(move |window, cx| {
                        let at = spot
                            .bounds()
                            .map_or_else(|| window.mouse_position(), |b| b.bottom_left());
                        by_key(at, window, cx)
                    });
                });
            });
        if let Some((at, menu)) = shown {
            this = this.child(
                deferred(
                    anchored()
                        .position(at)
                        .snap_to_window_with_margin(px(8.))
                        .child(menu),
                )
                .with_priority(1),
            );
        }
        this
    }
}

impl<E: InteractiveElement + ParentElement> KoanContextMenu for E {}

/// Where a set of buttons keeps the menu one of them has open: the
/// transport's sleep, output and EQ, a toolbar's menus.
///
/// A trigger toggles its menu. A menu closes itself on any press outside
/// it, and the trigger is outside it, so without care a click meant to
/// close would close the menu on its press and open it again on its
/// release. The slot notes, as the press begins and before the menu hears
/// it, whether the trigger's menu was open then; a click from that press
/// only closes it. The Mac's menus had to be fixed for the same thing.
pub struct MenuSlot<K> {
    open: Option<(K, Entity<Menu>, Subscription)>,
    /// The menu that was open when the current press began.
    pressed: Option<K>,
}

impl<K> Default for MenuSlot<K> {
    fn default() -> Self {
        Self {
            open: None,
            pressed: None,
        }
    }
}

impl<K: Copy + PartialEq + 'static> MenuSlot<K> {
    pub fn is_open(&self, which: K) -> bool {
        self.open.as_ref().is_some_and(|(open, ..)| *open == which)
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// The menu `which` opened, while it is open: to refill it as what it
    /// shows changes.
    pub fn menu(&self, which: K) -> Option<&Entity<Menu>> {
        self.open
            .as_ref()
            .filter(|(open, ..)| *open == which)
            .map(|(_, menu, _)| menu)
    }

    /// The trigger for `which`: `button` with the press noted and the click
    /// toggling, the menu `build` makes opening beneath it (`None` when there
    /// is nothing to show). `slot` reaches this slot inside the view that
    /// holds it.
    pub fn trigger<V: 'static, E>(
        &self,
        which: K,
        button: E,
        slot: fn(&mut V) -> &mut MenuSlot<K>,
        build: impl Fn(&mut V, &mut Window, &mut Context<V>) -> Option<Entity<Menu>> + 'static,
        cx: &mut Context<V>,
    ) -> Div
    where
        E: StatefulInteractiveElement + IntoElement + 'static,
    {
        self.trigger_at(false, which, button, slot, build, cx)
    }

    /// `trigger` for a button along the top of the window, a toolbar's: the
    /// menu opens below it, its left edge at the button's.
    pub fn trigger_below<V: 'static, E>(
        &self,
        which: K,
        button: E,
        slot: fn(&mut V) -> &mut MenuSlot<K>,
        build: impl Fn(&mut V, &mut Window, &mut Context<V>) -> Option<Entity<Menu>> + 'static,
        cx: &mut Context<V>,
    ) -> Div
    where
        E: StatefulInteractiveElement + IntoElement + 'static,
    {
        self.trigger_at(true, which, button, slot, build, cx)
    }

    fn trigger_at<V: 'static, E>(
        &self,
        below: bool,
        which: K,
        button: E,
        slot: fn(&mut V) -> &mut MenuSlot<K>,
        build: impl Fn(&mut V, &mut Window, &mut Context<V>) -> Option<Entity<Menu>> + 'static,
        cx: &mut Context<V>,
    ) -> Div
    where
        E: StatefulInteractiveElement + IntoElement + 'static,
    {
        let button = button
            .capture_any_mouse_down(cx.listener(move |view, _: &MouseDownEvent, _, _| {
                let slot = slot(view);
                slot.pressed = slot.open.as_ref().map(|(open, ..)| *open);
            }))
            .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                let was_open = slot(view).pressed.take() == Some(which);
                slot(view).open = None;
                if !was_open && let Some(menu) = build(view, window, cx) {
                    let dismissed = cx.subscribe(&menu, move |view, _, _: &DismissEvent, cx| {
                        slot(view).open = None;
                        cx.notify();
                    });
                    menu.focus_handle(cx).focus(window, cx);
                    slot(view).open = Some((which, menu, dismissed));
                }
                cx.notify();
            }));
        let open = self
            .open
            .as_ref()
            .filter(|(open, ..)| *open == which)
            .map(|(_, menu, _)| menu.clone());
        div().relative().child(button).children(open.map(|menu| {
            if below {
                // Pinned under the button; the anchor keeps it in the window.
                return div()
                    .absolute()
                    .top_full()
                    .left_0()
                    .child(
                        deferred(
                            anchored()
                                .anchor(Anchor::TopLeft)
                                .snap_to_window()
                                .child(menu),
                        )
                        .with_priority(1),
                    )
                    .into_any_element();
            }
            deferred(
                anchored()
                    .anchor(Anchor::BottomRight)
                    .snap_to_window()
                    .child(menu),
            )
            .with_priority(1)
            .into_any_element()
        }))
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;

    use super::*;
    // `gpui_kit::*` brings GPUI's `test` attribute, which would expand its
    // own `#[test]` into itself.
    use core::prelude::v1::test;

    struct Page {
        chosen: Rc<Cell<Option<&'static str>>>,
    }

    impl Render for Page {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let chosen = self.chosen.clone();
            div()
                .size_full()
                .debug_selector(|| "page".into())
                .koan_context_menu("page-menu", window, cx, move |menu, window, cx| {
                    let chosen = chosen.clone();
                    menu.item(MenuEntry::new("play").on_click(|_, _, _| {}))
                        .submenu("add to playlist", window, cx, move |menu, _, _| {
                            let (first, second) = (chosen.clone(), chosen.clone());
                            menu.item(
                                MenuEntry::data("Morning")
                                    .on_click(move |_, _, _| first.set(Some("Morning"))),
                            )
                            .item(
                                MenuEntry::data("Evening")
                                    .on_click(move |_, _, _| second.set(Some("Evening"))),
                            )
                        })
                })
        }
    }

    /// A playlist chosen with the pointer from "add to playlist" runs its
    /// choice: the press in the submenu, drawn outside the menu it opened
    /// from, closes neither before the click completes.
    #[gpui_kit::test]
    fn a_choice_in_a_submenu_runs(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let chosen = Rc::new(Cell::new(None));
        let held = chosen.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Page { chosen: held });
        let cx: &mut gpui_kit::VisualTestContext = cx;
        cx.run_until_parked();
        let page = cx.debug_bounds("page").expect("drawn");
        cx.simulate_mouse_down(
            page.origin + point(px(20.), px(20.)),
            MouseButton::Right,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            page.origin + point(px(20.), px(20.)),
            MouseButton::Right,
            Modifiers::default(),
        );
        cx.run_until_parked();
        let opener = cx
            .debug_bounds("menu: add to playlist")
            .expect("the menu opened");
        cx.simulate_mouse_move(opener.center(), None, Modifiers::default());
        cx.run_until_parked();
        let evening = cx
            .debug_bounds("menu: Evening")
            .expect("the submenu opened");
        cx.simulate_mouse_move(evening.center(), None, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_click(evening.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(chosen.get(), Some("Evening"));
        assert!(
            cx.debug_bounds("menu: Evening").is_none(),
            "the menus close once something is chosen"
        );
    }

    fn rows(menu: Menu, names: &[&'static str]) -> Menu {
        names.iter().fold(menu, |menu, name| {
            menu.item(MenuEntry::data(*name).on_click(|_, _, _| {}))
        })
    }

    /// Refilled as renderers arrive, the menu keeps the row highlighted
    /// while it is still there, and drops the highlight when it is gone.
    #[gpui_kit::test]
    fn refill_keeps_the_highlight_while_its_row_remains(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let window = cx.add_empty_window();
        let menu = window
            .update(|window, cx| Menu::build(window, cx, |menu, _, _| rows(menu, &["mac", "tv"])));
        window.update(|_, cx| {
            menu.update(cx, |menu, cx| {
                menu.highlight(Some(1), cx);
                menu.refill(cx, |menu, _| {
                    rows(menu, &["mac", "tv", "amp"]).note("plays the originals")
                });
                assert_eq!(menu.highlighted, Some(1));
                assert_eq!(menu.entries.len(), 4);
                menu.refill(cx, |menu, _| rows(menu, &["mac"]));
                assert_eq!(menu.highlighted, None);
            })
        });
    }
}

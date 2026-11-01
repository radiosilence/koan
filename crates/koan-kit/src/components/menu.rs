//! The theme's menu: the panel and its rows as a view of its own, opened as
//! a context menu at the pointer or below a button. Up and Down move through
//! it, Right and Return open a submenu, Return chooses, Escape and a click
//! outside close it. gpui-component's `PopupMenu` draws its own padding,
//! corners and type, which no theme setting reaches; this is the Mac's
//! `KoanMenu` instead.

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{MenuPanel, MenuRow, MenuSeparator};
use crate::theme::{Icon, Look, Space};

type Run = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type Build = Rc<dyn Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu>;

/// An entry of a menu, built as `PopupMenuItem` is.
pub struct MenuEntry {
    label: SharedString,
    data: bool,
    icon: Option<Icon>,
    checked: Option<bool>,
    destructive: bool,
    disabled: bool,
    run: Option<Run>,
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
            run: None,
        }
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
    Submenu {
        label: SharedString,
        build: Build,
        open: Option<Entity<Menu>>,
    },
}

pub struct Menu {
    entries: Vec<Entry>,
    highlighted: Option<usize>,
    focus: FocusHandle,
    _submenu: Option<Subscription>,
}

impl EventEmitter<DismissEvent> for Menu {}

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
                _submenu: None,
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

    /// A rule between groups; one at the start, at the end or after another
    /// is left out.
    pub fn separator(mut self) -> Self {
        if !matches!(self.entries.last(), None | Some(Entry::Separator)) {
            self.entries.push(Entry::Separator);
        }
        self
    }

    pub fn submenu(
        mut self,
        text: impl Into<SharedString>,
        _: &mut Window,
        _: &mut Context<Menu>,
        build: impl Fn(Menu, &mut Window, &mut Context<Menu>) -> Menu + 'static,
    ) -> Self {
        self.entries.push(Entry::Submenu {
            label: text.into(),
            build: Rc::new(build),
            open: None,
        });
        self
    }

    fn choosable(&self, index: usize) -> bool {
        match &self.entries[index] {
            Entry::Item(item) => !item.disabled && item.run.is_some(),
            Entry::Submenu { .. } => true,
            Entry::Separator => false,
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
        self._submenu = None;
        cx.notify();
    }

    fn open_submenu(
        &mut self,
        index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Entry::Submenu { build, open, .. }) = self.entries.get_mut(index) else {
            return;
        };
        if open.is_none() {
            let build = build.clone();
            let child = Menu::build(window, cx, move |menu, window, cx| build(menu, window, cx));
            // Choosing in the submenu closes this menu too.
            self._submenu =
                Some(cx.subscribe(&child, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent)));
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

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        match &self.entries[index] {
            Entry::Item(item) if !item.disabled => {
                if let Some(run) = item.run.clone() {
                    cx.emit(DismissEvent);
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
            "escape" | "left" => cx.emit(DismissEvent),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let menu = cx.entity();
        let mut panel = MenuPanel::new();
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
                    let menu = menu.clone();
                    row = row.on_click(move |_, window, cx| {
                        menu.update(cx, |menu, cx| menu.choose(index, window, cx))
                    });
                    panel.child(div().id(("hover", index)).on_hover(hovered).child(row))
                }
                Entry::Submenu { label, open, .. } => {
                    let row = MenuRow::new(("entry", index), label)
                        .highlighted(highlighted)
                        .opens(true);
                    panel.child(
                        div()
                            .id(("hover", index))
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
        div()
            .track_focus(&self.focus)
            .key_context("KoanMenu")
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .occlude()
            .child(panel)
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
        let state = window.use_keyed_state(id.into(), cx, |_, _| Open::default());
        let shown = state
            .read(cx)
            .0
            .as_ref()
            .map(|(at, menu, _)| (*at, menu.clone()));
        let build = Rc::new(build);
        let opener = state.clone();
        let mut this = self.on_mouse_down(MouseButton::Right, move |event, window, cx| {
            let build = build.clone();
            let menu = Menu::build(window, cx, move |menu, window, cx| build(menu, window, cx));
            // Nothing to act on, nothing to open.
            if menu.read(cx).entries.is_empty() {
                return;
            }
            let at = event.position;
            opener.update(cx, |open, cx| {
                let closing = cx.subscribe(&menu, |open: &mut Open, _, _: &DismissEvent, cx| {
                    open.0 = None;
                    cx.notify();
                });
                open.0 = Some((at, menu.clone(), closing));
                cx.notify();
            });
            window.focus(&menu.focus_handle(cx), cx);
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

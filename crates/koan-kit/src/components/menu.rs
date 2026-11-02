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
            detail: None,
            rich: false,
            reach: None,
            trailing: Vec::new(),
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
    Submenu {
        label: SharedString,
        icon: Option<Icon>,
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
            Entry::Separator | Entry::Heading(_) => false,
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
                Entry::Heading(title) => panel.child(
                    div()
                        .px(look.space(Space::M))
                        .pt(look.space(Space::Xs))
                        .text_role(crate::theme::TypeRole::Fine, &look)
                        .text_color(look.palette.muted.hsla())
                        .child(title.clone()),
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
                    let menu = menu.clone();
                    row = row.on_click(move |_, window, cx| {
                        menu.update(cx, |menu, cx| menu.choose(index, window, cx))
                    });
                    panel.child(div().id(("hover", index)).on_hover(hovered).child(row))
                }
                Entry::Submenu {
                    label, icon, open, ..
                } => {
                    let mut row = MenuRow::new(("entry", index), label)
                        .highlighted(highlighted)
                        .opens(true);
                    if let Some(icon) = icon {
                        row = row.icon(*icon);
                    }
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
            deferred(
                anchored()
                    .anchor(Anchor::BottomRight)
                    .snap_to_window()
                    .child(menu),
            )
            .with_priority(1)
        }))
    }
}

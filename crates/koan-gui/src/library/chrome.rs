//! What the library's pages share around their content: the header with its
//! count and actions, the filter field, the filter panel and sort menus, the
//! select toggle and the bar a pick acts through. Drawn in the kōan look:
//! square, flat, lowercase, one hairline where a region needs one.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::BrowseChoices;

use super::browse::{self, Browse};
use super::drag::Playable;
use super::selection::Selection;
use super::tokens::{self, Paint, Type, Typeset};
use super::{Lib, actions};
use crate::components::{
    Button, ButtonKind, EmptyState, FormRow, MenuPanel, MenuRow, TextField, Toggle,
};
use crate::theme::{Icon, KoanIcon, Space};

/// "1 album", "5,123 albums".
pub fn count(n: u64, one: &str, many: &str) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{grouped} {}", if n == 1 { one } else { many })
}

/// A library page's header: its title with the count under it, and its
/// actions trailing, top-aligned on the grid's margin.
pub fn page_header(
    p: &Paint,
    title: &'static str,
    detail: String,
    actions: impl IntoElement,
) -> Div {
    div()
        .flex()
        .items_start()
        .justify_between()
        .px(px(super::tile::MARGIN))
        .pt(p.space(Space::L))
        .gap(p.space(Space::L))
        .child(
            div()
                .flex()
                .flex_col()
                .child(div().typeset(p, Type::Title, p.strong).child(title))
                .child(div().typeset(p, Type::Meta, p.muted).child(detail)),
        )
        .child(actions)
}

/// An empty listing, centred in the page.
pub fn empty_state(icon: Icon, title: &'static str, detail: &'static str) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(EmptyState::new(icon, title).detail(detail))
}

/// A text button: an app word in `muted`, `ink` under the pointer.
pub fn text_button(id: impl Into<ElementId>, label: &str) -> Button {
    Button::new(id, ButtonKind::Text).label(label)
}

pub fn select_toggle(
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    text_button("select", if active { "done" } else { "select" }).on_click(on_click)
}

/// The filter field: the sidebar's search field's look, no box, the glyph and
/// a lowercase prompt in `muted`. Narrows the listing as it is typed.
pub struct SearchField {
    input: Entity<InputState>,
    _subscription: Subscription,
}

impl SearchField {
    pub fn new(browse: Entity<Browse>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial = browse.read(cx).search.clone();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("filter");
            state.set_value(initial, window, cx);
            state
        });
        let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                browse.update(cx, |b, cx| b.set_search(value, cx));
            }
        });
        Self {
            input,
            _subscription: subscription,
        }
    }
}

impl Render for SearchField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        div()
            .flex()
            .items_center()
            .gap(p.space(Space::Xs))
            .w(p.sized(200.0))
            .child(
                KoanIcon::new(Icon::Search)
                    .role(Type::Control)
                    .tone(crate::theme::Tone::Muted),
            )
            .child(TextField::new(&self.input).bare())
    }
}

/// A control that opens a panel: what a popover takes as its trigger.
#[derive(IntoElement)]
pub struct Trigger {
    id: ElementId,
    label: SharedString,
    open: bool,
}

impl Trigger {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            open: false,
        }
    }
}

impl gpui_kit::base::Selectable for Trigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl RenderOnce for Trigger {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        div()
            .id(self.id)
            .cursor_pointer()
            .typeset(p, Type::Control, if self.open { p.ink } else { p.muted })
            .hover(|s| s.text_color(p.ink))
            .child(self.label)
    }
}

/// The square panel a popover opens: `bg`, a hairline `rule`, no arrow.
fn panel(p: &Paint) -> Div {
    div()
        .bg(p.bg)
        .border_1()
        .border_color(p.rule)
        .typeset(p, Type::Body, p.ink)
}

/// A row in a menu panel for a choice that is data (a genre, a codec, a
/// playlist's name), which keeps its case; the theme's `MenuRow` writes the
/// app's own words lowercase.
fn choice_row(
    p: &Paint,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    ticked: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(p.space(Space::S))
        .px(p.space(Space::M))
        .py(p.space(Space::Xs))
        .cursor_pointer()
        .typeset(p, Type::Control, if ticked { p.accent } else { p.ink })
        .hover(|s| s.bg(p.surface))
        .child(div().w(px(12.0)).child(if ticked { "✓" } else { "" }))
        .child(label.into())
}

pub fn album_sort_button(lib: &Lib, cx: &App) -> impl IntoElement {
    let current = lib.browse.read(cx).effective_album_sort();
    let browse = lib.browse.clone();
    Popover::new("album-sort")
        .appearance(false)
        .arrow(false)
        .trigger(Trigger::new(
            "album-sort-trigger",
            format!("sort: {}", browse::album_sort_label(current)),
        ))
        .content(move |_, _, cx| {
            let b = browse.read(cx);
            let current = b.effective_album_sort();
            let sorts = browse::album_sorts(&b.filter, b.search().is_some());
            let state = cx.weak_entity();
            MenuPanel::new().children(sorts.into_iter().enumerate().map(|(i, sort)| {
                let (browse, state) = (browse.clone(), state.clone());
                MenuRow::new(("album-sort", i), browse::album_sort_label(sort))
                    .checked(sort == current)
                    .on_click(move |_, window, cx| {
                        browse.update(cx, |b, cx| {
                            // Random again is a new shuffle.
                            if sort == koan_ffi::AlbumSort::Random && b.album_sort == sort {
                                b.reshuffle(cx);
                            }
                            b.update(cx, |b| b.album_sort = sort);
                        });
                        state.update(cx, |s, cx| s.dismiss(window, cx)).ok();
                    })
            }))
        })
}

/// Shuffle again, under the random order: its own button beside the sort,
/// since a new deal is something tried until one is liked, and a menu makes
/// each try two clicks.
pub fn reshuffle_button(lib: &Lib, cx: &App) -> Option<Button> {
    let random = lib.browse.read(cx).effective_album_sort() == koan_ffi::AlbumSort::Random;
    let browse = lib.browse.clone();
    random.then(|| {
        Button::new("album-reshuffle", ButtonKind::Text)
            .icon(crate::theme::Icon::Reshuffle)
            .label("shuffle again")
            .on_click(move |_, _, cx| browse.update(cx, |b, cx| b.reshuffle(cx)))
    })
}

pub fn track_sort_button(lib: &Lib, cx: &App) -> impl IntoElement {
    let current = lib.browse.read(cx).effective_track_sort();
    let browse = lib.browse.clone();
    Popover::new("track-sort")
        .appearance(false)
        .arrow(false)
        .trigger(Trigger::new(
            "track-sort-trigger",
            format!("sort: {}", browse::track_sort_label(current)),
        ))
        .content(move |_, _, cx| {
            let b = browse.read(cx);
            let current = b.effective_track_sort();
            let sorts = browse::track_sorts(&b.filter, b.search().is_some());
            let state = cx.weak_entity();
            MenuPanel::new().children(sorts.into_iter().enumerate().map(|(i, sort)| {
                let (browse, state) = (browse.clone(), state.clone());
                MenuRow::new(("track-sort", i), browse::track_sort_label(sort))
                    .checked(sort == current)
                    .on_click(move |_, window, cx| {
                        browse.update(cx, |b, cx| b.update(cx, |b| b.track_sort = sort));
                        state.update(cx, |s, cx| s.dismiss(window, cx)).ok();
                    })
            }))
        })
}

pub fn filter_button(panel: &Entity<FilterPanel>, lib: &Lib, cx: &App) -> impl IntoElement {
    let active = lib.browse.read(cx).active_filters();
    let panel = panel.clone();
    Popover::new("filters")
        .appearance(false)
        .arrow(false)
        .trigger(Trigger::new(
            "filters-trigger",
            if active > 0 {
                format!("filters ({active})")
            } else {
                "filters".into()
            },
        ))
        .content(move |_, _, _| panel.clone())
}

/// The browsers' filters, at a fixed 300 points wide: the toggles, then codec
/// and genre and the years, then reset. One panel for the album, artist and
/// track browsers, which share the filter.
pub struct FilterPanel {
    lib: Lib,
    /// Lossless applies to records only, so the track browser leaves it out.
    lossless: bool,
    choices: Option<BrowseChoices>,
    year_from: Entity<InputState>,
    year_to: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl FilterPanel {
    pub fn new(lib: Lib, lossless: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = lib.browse.read(cx).filter.clone();
        let year = |value: Option<i32>,
                    placeholder: &'static str,
                    window: &mut Window,
                    cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                if let Some(v) = value {
                    state.set_value(v.to_string(), window, cx);
                }
                state
            })
        };
        let year_from = year(filter.year_from, "from", window, cx);
        let year_to = year(filter.year_to, "to", window, cx);
        let watch = |input: &Entity<InputState>, from: bool, cx: &mut Context<Self>| {
            cx.subscribe(input, move |this, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let value = input.read(cx).value().trim().parse::<i32>().ok();
                this.lib.browse.update(cx, |b, cx| {
                    let slot = if from {
                        b.filter.year_from
                    } else {
                        b.filter.year_to
                    };
                    if slot != value {
                        b.update(cx, |b| {
                            if from {
                                b.filter.year_from = value
                            } else {
                                b.filter.year_to = value
                            }
                        });
                    }
                });
            })
        };
        let subscriptions = vec![
            watch(&year_from, true, cx),
            watch(&year_to, false, cx),
            cx.observe(&lib.browse, |_, _, cx| cx.notify()),
        ];
        let engine = lib.engine.clone();
        cx.spawn(async move |this, cx| {
            let choices = engine.browse_choices().await;
            this.update(cx, |this, cx| match choices {
                Ok(c) => {
                    this.choices = Some(c);
                    cx.notify();
                }
                Err(e) => log::warn!("browse choices: {e}"),
            })
            .ok();
        })
        .detach();
        Self {
            lib,
            lossless,
            choices: None,
            year_from,
            year_to,
            _subscriptions: subscriptions,
        }
    }

    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        set: fn(&mut Browse, bool),
    ) -> impl IntoElement {
        let browse = self.lib.browse.clone();
        Toggle::new(id, label, on)
            .on_change(move |on, _, cx| browse.update(cx, |b, cx| b.update(cx, |b| set(b, *on))))
    }

    fn picker(
        &self,
        id: &'static str,
        label: &'static str,
        current: Option<String>,
        options: Vec<String>,
        set: fn(&mut Browse, Option<String>),
    ) -> impl IntoElement {
        let browse = self.lib.browse.clone();
        FormRow::new(label).control(
            Popover::new(id)
                .appearance(false)
                .arrow(false)
                .trigger(Trigger::new(
                    SharedString::from(format!("{id}-trigger")),
                    current.clone().unwrap_or_else(|| "any".into()),
                ))
                .content(move |_, window, cx| {
                    let p = &Paint::of(window, cx);
                    let rows = std::iter::once(None).chain(options.iter().cloned().map(Some));
                    MenuPanel::new().children(rows.enumerate().map(|(i, option)| {
                        let browse = browse.clone();
                        let ticked = option == current;
                        let label = option.clone().unwrap_or_else(|| "any".into());
                        choice_row(p, (id, i), label, ticked).on_click(cx.listener(
                            move |state, _, window, cx| {
                                let option = option.clone();
                                browse.update(cx, |b, cx| b.update(cx, |b| set(b, option)));
                                state.dismiss(window, cx);
                            },
                        ))
                    }))
                }),
        )
    }
}

impl Render for FilterPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let f = self.lib.browse.read(cx).filter.clone();
        let (codecs, genres) = self
            .choices
            .as_ref()
            .map(|c| (c.codecs.clone(), c.genres.clone()))
            .unwrap_or_default();
        let rule = || div().h(tokens::hairline()).bg(p.row_rule());
        let browse = self.lib.browse.clone();
        let (from, to) = (self.year_from.clone(), self.year_to.clone());
        panel(p)
            .w(px(300.0))
            .p(p.space(Space::M))
            .flex()
            .flex_col()
            .gap(p.space(Space::S))
            .child(self.toggle("f-fav", "favourites", f.favourites, |b, on| {
                b.filter.favourites = on
            }))
            .child(
                self.toggle("f-recent", "recently played", f.recent, |b, on| {
                    b.filter.recent = on
                }),
            )
            .child(self.toggle("f-down", "downloaded", f.downloaded, |b, on| {
                b.filter.downloaded = on
            }))
            .when(self.lossless, |d| {
                d.child(self.toggle("f-lossless", "lossless", f.lossless, |b, on| {
                    b.filter.lossless = on
                }))
            })
            .child(rule())
            .child(
                self.picker("f-codec", "codec", f.codec.clone(), codecs, |b, v| {
                    b.filter.codec = v
                }),
            )
            .child(
                self.picker("f-genre", "genre", f.genre.clone(), genres, |b, v| {
                    b.filter.genre = v
                }),
            )
            .child(
                FormRow::new("years").control(
                    div()
                        .flex()
                        .gap(p.space(Space::S))
                        .child(
                            div()
                                .w(p.sized(64.0))
                                .child(TextField::new(&self.year_from)),
                        )
                        .child(div().w(p.sized(64.0)).child(TextField::new(&self.year_to))),
                ),
            )
            .child(rule())
            .child(
                div().flex().justify_end().child(
                    Button::new("f-reset", ButtonKind::Bordered)
                        .label("reset")
                        .on_click(move |_, window, cx| {
                            browse.update(cx, |b, cx| {
                                b.update(cx, |b| b.filter = Default::default())
                            });
                            from.update(cx, |i, cx| i.set_value("", window, cx));
                            to.update(cx, |i, cx| i.set_value("", window, cx));
                        }),
                ),
            )
    }
}

/// A page whose items can be picked.
pub trait Picking: Sized + 'static {
    fn selection_mut(&mut self) -> &mut Selection;
}

/// Along the foot of a page while picking: how many, and the verbs. Each verb
/// ends the mode.
pub fn selection_bar<V: Picking>(
    p: &Paint,
    lib: &Lib,
    selection: &Selection,
    page: Entity<V>,
    cx: &App,
) -> impl IntoElement {
    let picked: Vec<Playable> = selection.picked().to_vec();
    let n = picked.len();
    let verb = |id: &'static str, label: &'static str, act: fn(&Lib, &[Playable], &mut App)| {
        let (lib, picked, page) = (lib.clone(), picked.clone(), page.clone());
        text_button(id, label).on_click(move |_, _, cx| {
            if picked.is_empty() {
                return;
            }
            act(&lib, &picked, cx);
            page.update(cx, |page, cx| {
                page.selection_mut().end();
                cx.notify();
            });
        })
    };
    let playlists = lib.feed.read(cx).playlists.clone();
    let (lib2, picked2, page2) = (lib.clone(), picked.clone(), page.clone());
    let done = page.clone();
    div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(super::tile::MARGIN))
        .py(p.space(Space::M))
        .border_t_1()
        .border_color(p.rule)
        .bg(p.bg)
        .child(
            div()
                .typeset(p, Type::Meta, p.ink)
                .child(count(n as u64, "picked", "picked")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(p.space(Space::L))
                .child(verb("sel-play", "play", |lib, p, cx| {
                    actions::play(lib, p, false, cx).detach()
                }))
                .child(verb("sel-next", "play next", |lib, p, cx| {
                    actions::queue(lib, p, koan_ffi::Placement::Next, cx).detach()
                }))
                .child(verb("sel-queue", "add to queue", |lib, p, cx| {
                    actions::queue(lib, p, koan_ffi::Placement::Append, cx).detach()
                }))
                .child(
                    Popover::new("sel-playlist")
                        .appearance(false)
                        .arrow(false)
                        .trigger(Trigger::new("sel-playlist-trigger", "add to playlist"))
                        .content(move |_, window, cx| {
                            let p = &Paint::of(window, cx);
                            if playlists.is_empty() {
                                return panel(p)
                                    .p(p.space(Space::M))
                                    .typeset(p, Type::Control, p.muted)
                                    .child("no playlists")
                                    .into_any_element();
                            }
                            MenuPanel::new()
                                .children(playlists.iter().enumerate().map(|(i, (id, name))| {
                                    let (lib, picked, page, id) =
                                        (lib2.clone(), picked2.clone(), page2.clone(), *id);
                                    choice_row(p, ("sel-pl", i), name.clone(), false).on_click(
                                        cx.listener(move |state, _, window, cx| {
                                            actions::add_to_playlist(&lib, id, &picked, cx);
                                            page.update(cx, |page, cx| {
                                                page.selection_mut().end();
                                                cx.notify();
                                            });
                                            state.dismiss(window, cx);
                                        }),
                                    )
                                }))
                                .into_any_element()
                        }),
                )
                .child(verb("sel-fav", "favourite", actions::favourite))
                .child(text_button("sel-done", "done").on_click(move |_, _, cx| {
                    done.update(cx, |page, cx| {
                        page.selection_mut().end();
                        cx.notify();
                    })
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::count;

    #[test]
    fn counts_are_grouped_and_pluralised() {
        assert_eq!(count(1, "album", "albums"), "1 album");
        assert_eq!(count(0, "album", "albums"), "0 albums");
        assert_eq!(count(5123, "album", "albums"), "5,123 albums");
        assert_eq!(count(1_234_567, "track", "tracks"), "1,234,567 tracks");
    }
}

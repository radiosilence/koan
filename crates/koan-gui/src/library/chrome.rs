//! What the library's pages share around their content: the header with its
//! count and actions, the filter field, the filter panel and sort menus, the
//! select toggle and the bar a pick acts through. Drawn in the kōan look:
//! square, flat, lowercase, one hairline where a region needs one.

use koan_kit::app::input::{InputEvent, InputState};
use koan_kit::app::*;
use koan_runtime::BrowseChoices;

use super::browse::{self, Browse};
use super::drag::Playable;
use super::selection::Selection;
use super::tokens::Paint;
use super::{Lib, actions};
use crate::components::{
    Button, ButtonKind, EmptyState, FormRow, MenuPanel, MenuRow, MenuSeparator, SelectionBar,
    TextField, Toggle, ToolbarButton, ToolbarMenu,
};
use crate::theme::{Icon, Space};

/// "1 album", "5,123 albums".
pub fn count(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { one } else { many })
}

/// "5,123": a number with its thousands marked.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

/// A library page's header: koan-kit's `PageHeader`, its title with the
/// count under it and its actions trailing, at the grid's margin.
pub fn page_header(
    p: &Paint,
    title: &'static str,
    detail: String,
    actions: impl IntoElement,
) -> Div {
    div()
        .px(crate::components::RowMetrics::page_margin(&p.look))
        .pt(crate::components::RowMetrics::page_top(&p.look))
        .child(
            crate::components::PageHeader::new(title)
                .detail(detail)
                .trailing(actions),
        )
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

/// What a browser does with the listing its last visit kept, against the
/// query and the library's version now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kept {
    /// The same listing: drawn at once, where the last visit left it.
    Draw,
    /// The same query over a library that has moved: not drawn, since its
    /// rows may no longer be the library's; the place is kept for the new
    /// listing.
    Wait,
    /// Another query (a filter changed on another page): neither the rows
    /// nor the place are this listing's.
    Forget,
}

/// See `Kept`. A version not yet known is taken as the same: the engine's
/// first report is a frame away, and the load it brings checks again.
pub fn kept<Q: PartialEq>(kept: &(Q, u64), query: &Q, version: Option<u64>) -> Kept {
    if kept.0 != *query {
        Kept::Forget
    } else if version.is_some_and(|v| v != kept.1) {
        Kept::Wait
    } else {
        Kept::Draw
    }
}

/// A listing that could not be read, why, and a way to ask again: what a
/// browser draws in place of "no albums yet" when the library did not answer.
pub fn failed_state(
    icon: Icon,
    why: &str,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .child(
            EmptyState::new(icon, "could not read the library")
                .warning()
                .detail(why.to_string()),
        )
        .child(text_button("retry-listing", "try again").on_click(retry))
}

/// A text button: an app word in `muted`, `ink` under the pointer.
pub fn text_button(id: impl Into<ElementId>, label: &str) -> Button {
    Button::new(id, ButtonKind::Text).label(label)
}

pub fn select_toggle(
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> ToolbarButton {
    ToolbarButton::new(
        "select",
        if active {
            Icon::Selecting
        } else {
            Icon::Select
        },
        if active { "done" } else { "select" },
    )
    .help(if active {
        "stop selecting (esc)".to_owned()
    } else {
        crate::platform::actions::help(
            "pick several to play or queue",
            &crate::platform::actions::SelectAll,
            cx,
        )
    })
    .selected(active)
    .on_click(on_click)
}

/// The filter field: the sidebar's search field's look, no box, the glyph and
/// a lowercase prompt in `muted`. Narrows the listing as it is typed.
pub struct SearchField {
    input: Entity<InputState>,
    _subscriptions: [Subscription; 2],
}

impl SearchField {
    /// `prompt` says what it narrows: "filter albums".
    pub fn new(
        browse: Entity<Browse>,
        prompt: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = browse.read(cx).search.clone();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(prompt);
            state.set_value(initial, window, cx);
            state
        });
        let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                browse.update(cx, |b, cx| b.set_search(value, cx));
            }
        });
        // The pages are cached views: the field redraws (focus, caret, each
        // keystroke) only when it is marked dirty, which takes its own view
        // noticing the input change, not only the input.
        let redraw = cx.observe(&input, |_, _, cx| cx.notify());
        Self {
            input,
            _subscriptions: [subscription, redraw],
        }
    }
}

impl Render for SearchField {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        crate::components::FilterField::new(&self.input)
    }
}

/// A browser's controls in the window's toolbar: the filter field first, then
/// what the caller adds. On a phone the field is the list's (`with_filter`)
/// and the bar holds the buttons alone.
pub fn browser_controls(
    search: &Entity<SearchField>,
    window: &Window,
    cx: &App,
) -> crate::components::ToolbarGroup {
    if crate::theme::Look::of(window, cx).compact {
        crate::components::ToolbarGroup::new()
    } else {
        crate::components::ToolbarGroup::new()
            .fills()
            .child(search.clone())
            .spacer()
    }
}

/// What a phone browser leads its list with: its filter field alone,
/// scrolled away with the list as an iOS list's search field is. The bar
/// names the page, so it draws no heading of its own.
pub fn with_filter(search: &Entity<SearchField>, p: &Paint) -> AnyElement {
    div()
        .px(crate::components::RowMetrics::page_margin(&p.look))
        .pt(p.space(Space::S))
        .child(search.clone())
        .into_any_element()
}

pub fn album_sort_button(lib: &Lib, cx: &App) -> impl IntoElement {
    let current = lib.browse.read(cx).effective_album_sort();
    let browse = lib.browse.clone();
    ToolbarMenu::new(
        "album-sort",
        ToolbarButton::new(
            "album-sort-trigger",
            Icon::Sort,
            format!("sort: {}", browse::album_sort_label(current)),
        ),
        move |dismiss, _, cx| {
            let b = browse.read(cx);
            let current = b.effective_album_sort();
            let sorts = browse::album_sorts(&b.filter, b.search().is_some());
            MenuPanel::new().children(sorts.into_iter().enumerate().map(|(i, sort)| {
                let (browse, dismiss) = (browse.clone(), dismiss.clone());
                MenuRow::new(("album-sort", i), browse::album_sort_label(sort))
                    .checked(sort == current)
                    .on_click(move |_, window, cx| {
                        dismiss(window, cx);
                        browse.update(cx, |b, cx| {
                            // Random again is a new shuffle.
                            if sort == koan_runtime::AlbumSort::Random && b.album_sort == sort {
                                b.reshuffle(cx);
                            }
                            b.update(cx, |b| b.album_sort = sort);
                        });
                    })
            }))
        },
    )
}

/// Shuffle again, under the random order: its own button beside the sort,
/// since a new deal is something tried until one is liked, and a menu makes
/// each try two clicks.
pub fn reshuffle_button(lib: &Lib, cx: &App) -> Option<ToolbarButton> {
    let random = lib.browse.read(cx).effective_album_sort() == koan_runtime::AlbumSort::Random;
    let browse = lib.browse.clone();
    random.then(|| {
        ToolbarButton::new("album-reshuffle", Icon::Reshuffle, "shuffle again")
            .on_click(move |_, _, cx| browse.update(cx, |b, cx| b.reshuffle(cx)))
    })
}

pub fn track_sort_button(lib: &Lib, cx: &App) -> impl IntoElement {
    let current = lib.browse.read(cx).effective_track_sort();
    let browse = lib.browse.clone();
    ToolbarMenu::new(
        "track-sort",
        ToolbarButton::new(
            "track-sort-trigger",
            Icon::Sort,
            format!("sort: {}", browse::track_sort_label(current)),
        ),
        move |dismiss, _, cx| {
            let b = browse.read(cx);
            let current = b.effective_track_sort();
            let sorts = browse::track_sorts(&b.filter, b.search().is_some());
            MenuPanel::new().children(sorts.into_iter().enumerate().map(|(i, sort)| {
                let (browse, dismiss) = (browse.clone(), dismiss.clone());
                MenuRow::new(("track-sort", i), browse::track_sort_label(sort))
                    .checked(sort == current)
                    .on_click(move |_, window, cx| {
                        dismiss(window, cx);
                        browse.update(cx, |b, cx| b.update(cx, |b| b.track_sort = sort));
                    })
            }))
        },
    )
}

pub fn filter_button(panel: &Entity<FilterPanel>, lib: &Lib, cx: &App) -> impl IntoElement {
    let active = lib.browse.read(cx).active_filters();
    let panel = panel.clone();
    crate::components::opens_panel(
        "filters",
        ToolbarButton::new(
            "filters-trigger",
            Icon::Filters,
            if active > 0 {
                format!("filters ({active})")
            } else {
                "filters".into()
            },
        ),
        move |_, _, _| panel.clone().into_any_element(),
    )
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
                // A year half typed is not a filter: one is applied as it is
                // completed or cleared, or when the field is left.
                let text = input.read(cx).value().trim().to_owned();
                let settled = match event {
                    InputEvent::Change => text.is_empty() || text.len() == 4,
                    InputEvent::Blur | InputEvent::PressEnter { .. } => true,
                    _ => false,
                };
                if !settled {
                    return;
                }
                let value = text.parse::<i32>().ok();
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
            // Redrawn as the fields are, as `SearchField` is.
            cx.observe(&year_from, |_, _, cx| cx.notify()),
            cx.observe(&year_to, |_, _, cx| cx.notify()),
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
        FormRow::new(label)
            .compact()
            .control(crate::components::opens_panel(
                id,
                crate::components::TextTrigger::new(
                    SharedString::from(format!("{id}-trigger")),
                    current.clone().unwrap_or_else(|| "any".into()),
                ),
                move |dismiss, _, _| {
                    let rows = std::iter::once(None).chain(options.iter().cloned().map(Some));
                    MenuPanel::new()
                        .children(rows.enumerate().map(|(i, option)| {
                            let (browse, dismiss) = (browse.clone(), dismiss.clone());
                            let ticked = option == current;
                            let row = match &option {
                                Some(label) => MenuRow::data((id, i), label.clone()),
                                None => MenuRow::new((id, i), "any"),
                            };
                            row.checked(ticked).on_click(move |_, window, cx| {
                                let option = option.clone();
                                browse.update(cx, |b, cx| b.update(cx, |b| set(b, option)));
                                dismiss(window, cx);
                            })
                        }))
                        .into_any_element()
                },
            ))
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
        let browse = self.lib.browse.clone();
        let (from, to) = (self.year_from.clone(), self.year_to.clone());
        crate::components::popover_panel(300.0, &p.look)
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
            .child(MenuSeparator)
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
                FormRow::new("years").compact().control(
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
            .child(MenuSeparator)
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
    _: &Paint,
    lib: &Lib,
    selection: &Selection,
    page: Entity<V>,
    cx: &App,
) -> impl IntoElement {
    let picked: Vec<Playable> = selection.picked().to_vec();
    let n = picked.len();
    let end = {
        let page = page.clone();
        move |cx: &mut App| {
            page.update(cx, |page, cx| {
                page.selection_mut().end();
                cx.notify();
            })
        }
    };
    let verb = |act: fn(&Lib, &[Playable], &mut App)| {
        let (lib, picked, end) = (lib.clone(), picked.clone(), end.clone());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            if picked.is_empty() {
                return;
            }
            act(&lib, &picked, cx);
            end(cx);
        }
    };
    let to_playlist = {
        let (lib, picked, end) = (lib.clone(), picked.clone(), end.clone());
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if picked.is_empty() {
                return;
            }
            let end = end.clone();
            let after: actions::After = Some(std::rc::Rc::new(move |cx: &mut App| end(cx)));
            super::pick_playlist::open(&lib, picked.as_slice().into(), after, window, cx);
        }
    };
    let organise = {
        let (lib, picked, end) = (lib.clone(), picked.clone(), end.clone());
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if picked.is_empty() {
                return;
            }
            actions::organise(&lib, &picked, window, cx);
            end(cx);
        }
    };
    // On a phone the verbs are icons over their words, sharing the width;
    // five written out in full would not fit along it.
    if cx.global::<crate::theme::KoanTheme>().touch {
        let fav_all =
            !picked.is_empty() && picked.iter().all(|i| lib.feed.read(cx).is_favourite(*i));
        let none = picked.is_empty();
        return SelectionBar::new(count(n as u64, "picked", "picked"))
            .icon_verb(
                "sel-play",
                Icon::Play,
                "play",
                none,
                verb(|lib, p, cx| actions::play(lib, p, false, cx).detach()),
            )
            .icon_verb(
                "sel-next",
                Icon::Next,
                "next",
                none,
                verb(|lib, p, cx| {
                    actions::queue(lib, p, koan_runtime::Placement::Next, cx).detach()
                }),
            )
            .icon_verb(
                "sel-queue",
                Icon::QueueSection,
                "queue",
                none,
                verb(|lib, p, cx| {
                    actions::queue(lib, p, koan_runtime::Placement::Append, cx).detach()
                }),
            )
            .icon_verb(
                "sel-playlist",
                Icon::Playlist,
                "playlist",
                none,
                to_playlist,
            )
            .icon_verb(
                "sel-fav",
                Icon::Favourite,
                if fav_all { "unlike" } else { "like" },
                none,
                verb(actions::favourite),
            )
            .on_done(move |_, _, cx| end(cx))
            .done_help("stop selecting");
    }
    SelectionBar::new(count(n as u64, "picked", "picked"))
        .verb_help(
            "sel-play",
            "play",
            "play the selection, replacing the queue",
            verb(|lib, p, cx| actions::play(lib, p, false, cx).detach()),
        )
        .verb(
            "sel-next",
            "play next",
            verb(|lib, p, cx| actions::queue(lib, p, koan_runtime::Placement::Next, cx).detach()),
        )
        .verb_help(
            "sel-queue",
            "add to queue",
            "add the selection to the end of the queue",
            verb(|lib, p, cx| actions::queue(lib, p, koan_runtime::Placement::Append, cx).detach()),
        )
        .verb("sel-playlist", "add to playlist…", to_playlist)
        // As the context menu says it: a pick of favourites only is
        // unfavourited.
        .verb(
            "sel-fav",
            if !picked.is_empty() && picked.iter().all(|i| lib.feed.read(cx).is_favourite(*i)) {
                "unfavourite"
            } else {
                "favourite"
            },
            verb(actions::favourite),
        )
        .when(!crate::mobile::handheld(), |bar| {
            bar.verb("sel-organise", "organise…", organise)
        })
        .on_done(move |_, _, cx| end(cx))
        .done_help("stop selecting (esc)")
}

#[cfg(test)]
mod kept_tests {
    use super::{Kept, kept};

    #[test]
    fn a_kept_listing_is_drawn_only_while_it_is_the_listing() {
        assert_eq!(kept(&("rock", 4), &"rock", Some(4)), Kept::Draw);
        assert_eq!(
            kept(&("rock", 4), &"rock", None),
            Kept::Draw,
            "not yet known"
        );
        assert_eq!(
            kept(&("rock", 4), &"rock", Some(5)),
            Kept::Wait,
            "the library moved"
        );
        assert_eq!(
            kept(&("rock", 4), &"jazz", Some(4)),
            Kept::Forget,
            "another filter"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    use koan_kit::app::test::TestAppContext;

    /// Typing in the filter field narrows the browsers: what is typed
    /// reaches the browse model they all listen to.
    #[koan_kit::app::test::test]
    fn typing_in_the_filter_field_sets_the_search(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
        });
        let browse = cx.new(|_| Browse::fresh());
        let watched = browse.clone();
        let (field, cx) = cx.add_window_view(move |window, cx| {
            SearchField::new(watched.clone(), "filter albums", window, cx)
        });
        cx.update(|window, cx| {
            let handle = field.read(cx).input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.simulate_input("lush");
        cx.run_until_parked();
        assert_eq!(
            browse.read_with(cx, |b, _| b.search()),
            Some("lush".to_string())
        );
    }

    #[test]
    fn counts_are_grouped_and_pluralised() {
        assert_eq!(count(1, "album", "albums"), "1 album");
        assert_eq!(count(0, "album", "albums"), "0 albums");
        assert_eq!(count(5123, "album", "albums"), "5,123 albums");
        assert_eq!(count(1_234_567, "track", "tracks"), "1,234,567 tracks");
    }
}

//! A record's page: the header with its cover, title, credits as links and
//! play, the one prominent action; then its tracks, under a heading per disc
//! where the record numbers by disc.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use koan_ffi::{Album, Track};
use koan_kit::app::*;
// virtualised lists and their scroll state, which GPUI owns and koan-kit's rows sit in.
use crate::components::{UniformListScrollHandle, uniform_list};

use super::covers::{Art, Covers, Key, Priority};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::rows::{self, RowPart, RowState, TrackList};
use super::selection::Selection;
use super::tile::Grid;
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::components::{Cover, CoverArt, RowKind, RowMetrics};
use crate::theme::Space;

/// A line of the list: a disc's heading, or a track and the number it shows.
enum Line {
    Disc(SharedString),
    Track { index: usize, number: SharedString },
}

/// Whether a record numbers by disc: any track on a disc past the first.
fn numbers_by_disc(tracks: &[Track]) -> bool {
    tracks.iter().any(|t| t.disc.unwrap_or(0) > 1)
}

/// The record's tracks as lines: under a heading per disc where it numbers by
/// disc, each row showing its own number (or its place in its disc where the
/// file has none); otherwise its place on the record.
fn lines(tracks: &[Track]) -> Vec<Line> {
    if !numbers_by_disc(tracks) {
        return (0..tracks.len())
            .map(|index| Line::Track {
                index,
                number: (index + 1).to_string().into(),
            })
            .collect();
    }
    let mut out = Vec::new();
    let mut disc: Option<Option<i32>> = None;
    let mut place = 0;
    for (index, track) in tracks.iter().enumerate() {
        let this = track.disc.filter(|d| *d > 0);
        if disc != Some(this) {
            disc = Some(this);
            place = 0;
            out.push(Line::Disc(match this {
                Some(d) => format!("disc {d}").into(),
                None => "other tracks".into(),
            }));
        }
        let number = track
            .track_number
            .filter(|n| *n > 0)
            .map_or(place + 1, |n| n as usize);
        out.push(Line::Track {
            index,
            number: number.to_string().into(),
        });
        place += 1;
    }
    out
}

/// How long a record's first read may take before the page says it is
/// still reading.
const SLOW_READ: std::time::Duration = std::time::Duration::from_millis(300);

pub struct AlbumPage {
    lib: Lib,
    album_id: i64,
    album: Option<Album>,
    tracks: Vec<Track>,
    lines: Vec<Line>,
    /// Read and found nothing: gone from the library, or the read failed.
    gone: Option<bool>,
    request: u64,
    loaded_version: Option<u64>,
    _load: Option<Task<()>>,
    /// The first read has taken long enough to say so: a record from a
    /// server can, where a local one lands within a frame or two.
    slow: bool,
    _slow: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    /// The keyboard's place among the tracks.
    cursor: crate::components::ListCursor,
    /// The list holds the keyboard, as of the last frame.
    keyed: bool,
    width: Rc<Cell<Option<Pixels>>>,
    selection: Selection,
    pick: Option<(u64, PlayableDrag)>,
    /// What the list's context menu acts on: the row last right-clicked.
    menu: Option<actions::MenuTarget>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl AlbumPage {
    pub fn new(lib: Lib, album_id: i64, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        let subscriptions = vec![
            // Cached under the window, and draws the accent.
            crate::theme::observe_accent(cx),
            cx.observe(&lib.feed, |this, _, cx| {
                this.load(cx);
                cx.notify();
            }),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.subscribe(&lib.covers, |this, _, gone: &super::covers::Evicted, cx| {
                let tracks: std::collections::HashSet<i64> = gone.tracks.iter().copied().collect();
                this.selection
                    .retain(|p| !matches!(p, Playable::Track(id) if tracks.contains(&id)));
                cx.notify();
            }),
            cx.on_release(move |this, cx| {
                this.lib.covers.update(cx, |c, cx| c.release(id, cx));
            }),
        ];
        let mut this = Self {
            lib,
            album_id,
            album: None,
            tracks: Vec::new(),
            lines: Vec::new(),
            gone: None,
            request: 0,
            loaded_version: None,
            _load: None,
            slow: false,
            _slow: None,
            scroll: UniformListScrollHandle::new(),
            cursor: Default::default(),
            keyed: false,
            width: Rc::new(Cell::new(None)),
            selection: Selection::default(),
            pick: None,
            menu: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.load(cx);
        // One timer for the first read alone, so a spinner shows only for a
        // read there was something to wait for.
        this._slow = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SLOW_READ).await;
            this.update(cx, |this, cx| {
                if this.album.is_none() && this.gone.is_none() {
                    this.slow = true;
                    cx.notify();
                }
            })
            .ok();
        }));
        this
    }

    /// The record and its tracks, read again when the library moves: a
    /// download landing writes a path onto one of these rows.
    fn load(&mut self, cx: &mut Context<Self>) {
        let version = self.lib.feed.read(cx).library_version;
        if version.is_none() || version == self.loaded_version {
            return;
        }
        self.loaded_version = version;
        self.request += 1;
        let request = self.request;
        let (engine, id) = (self.lib.engine.clone(), self.album_id);
        self._load = Some(cx.spawn(async move |this, cx| {
            let page = engine.album_page(id).await;
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                match page {
                    Ok(page) if page.album.is_some() => {
                        this.lines = lines(&page.tracks);
                        this.album = page.album;
                        this.tracks = page.tracks;
                        this.gone = None;
                    }
                    Ok(_) => this.gone = Some(false),
                    Err(e) => {
                        log::warn!("album {id}: {e}");
                        this.gone = Some(true);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        self.loaded_version = None;
        self.load(cx);
    }

    fn pick(&mut self) -> Option<PlayableDrag> {
        if self.selection.picked().is_empty() {
            return None;
        }
        let revision = self.selection.revision();
        if self.pick.as_ref().is_none_or(|(r, _)| *r != revision) {
            let picked = self.selection.picked();
            let label = chrome::count(picked.len() as u64, "track", "tracks");
            self.pick = Some((
                revision,
                PlayableDrag::new(picked, label).with_sleeve(Some(self.album_id)),
            ));
        }
        self.pick.as_ref().map(|(_, drag)| drag.clone())
    }

    /// One height for every line, the Mac's track row's: room for a credit
    /// under the title whether or not a row names one.
    fn row_kind(&self) -> RowKind {
        RowKind::Text
    }

    fn line(
        &mut self,
        p: &Paint,
        i: usize,
        lead_width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = self.row_kind();
        let height = crate::components::RowMetrics::height(kind, &p.look);
        let (index, number) = match &self.lines[i] {
            Line::Disc(title) => {
                return div()
                    .w_full()
                    .h(height)
                    .flex()
                    .items_end()
                    .px(RowMetrics::page_margin(&p.look))
                    .pb(p.space(Space::Xs))
                    .typeset(p, Type::Fine, p.ink)
                    .child(title.clone())
                    .into_any_element();
            }
            Line::Track { index, number } => (*index, number.clone()),
        };
        let (id, title) = {
            let t = &self.tracks[index];
            (t.id, t.title.clone())
        };
        let item = Playable::Track(id);
        let selected = self.selection.contains(item);
        let drag = match (selected, self.pick()) {
            (true, Some(pick)) => pick,
            _ => PlayableDrag::new(&[item], title).with_sleeve(Some(self.album_id)),
        };
        let feed = self.lib.feed.read(cx);
        let state = RowState {
            lead: number,
            lead_width,
            current: feed.playing_track == Some(id),
            favourite: feed.is_favourite(item),
            selected,
            gathered: false,
            transfer: feed.transfers.get(&id).copied(),
            drag,
            kind,
            note: None,
            cursor: self.keyed && self.cursor.at == Some(index),
        };
        rows::track_row(p, &self.tracks[index], index, state, None, None, cx)
    }

    fn header(
        &mut self,
        p: &Paint,
        _grid: Grid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let album = self.album.clone();
        let side = RowMetrics::header_art(&p.look);
        let key = Key::new(Art::Album(self.album_id), side, window.scale_factor());
        let owner = cx.entity_id();
        let sleeve = self.lib.covers.update(cx, |c: &mut Covers, _| {
            c.request(owner, [(key, Priority::Visible)]);
            c.sleeve(key)
        });
        let total: i64 = self.tracks.iter().filter_map(|t| t.duration_ms).sum();
        let item = Playable::Album(self.album_id);
        let mut details = div().flex().flex_wrap();
        if let Some(album) = &album {
            for (i, part) in album.artist_credit.iter().enumerate() {
                let span = div().id(("page-credit", i)).child(part.text.clone());
                details = details.child(match part.artist_id {
                    Some(artist) => {
                        let nav = self.lib.nav.clone();
                        let ink = p.ink;
                        span.cursor_pointer()
                            .hover(move |s| s.underline().text_color(ink))
                            .on_click(move |_, _, cx| {
                                nav.update(cx, |n, cx| n.open(Page::Artist(artist), cx))
                            })
                            .into_any_element()
                    }
                    None => span.into_any_element(),
                });
            }
            let mut rest = Vec::new();
            if let Some(year) = album.year {
                rest.push(year.to_string());
            }
            if let Some(codec) = &album.codec {
                rest.push(codec.to_uppercase());
            }
            if total > 0 {
                rest.push(super::format::duration(Some(total)));
            }
            if !rest.is_empty() {
                details = details.child(format!(" · {}", rest.join(" · ")));
            }
        }
        let title = album.as_ref().map(|a| a.title.clone()).unwrap_or_default();
        div()
            .px(RowMetrics::page_margin(&p.look))
            .pt(RowMetrics::page_top(&p.look))
            .pb(RowMetrics::header_gap(&p.look))
            .child(
                super::header::playable_header(&self.lib, item, title.clone(), cx)
                    .art(
                        div()
                            .id("album-sleeve")
                            .cursor_pointer()
                            .tooltip(|window, cx| {
                                crate::components::Tooltip::view("show artwork".into(), window, cx)
                            })
                            .on_click({
                                let (album_id, credit) = (
                                    self.album_id,
                                    album.as_ref().map(|a| {
                                        SharedString::from(
                                            a.artist_credit
                                                .iter()
                                                .map(|c| c.text.as_str())
                                                .collect::<String>(),
                                        )
                                    }),
                                );
                                move |_, window, cx| {
                                    crate::artwork::show(
                                        album_id,
                                        title.clone(),
                                        credit.clone(),
                                        window,
                                        cx,
                                    )
                                }
                            })
                            .child(Cover::new(CoverArt::from(sleeve), side)),
                    )
                    .detail(details),
            )
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let order: Vec<Playable> = self.tracks.iter().map(|t| Playable::Track(t.id)).collect();
        self.selection.select_all(&order);
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.is_clear() {
            self.selection.end();
            cx.notify();
        }
    }
}

impl actions::Menus for AlbumPage {
    fn lib(&self) -> &Lib {
        &self.lib
    }

    fn menu_target(&mut self) -> &mut Option<actions::MenuTarget> {
        &mut self.menu
    }

    fn menu_target_ref(&self) -> &Option<actions::MenuTarget> {
        &self.menu
    }
}

impl TrackList for AlbumPage {
    fn row_tracks(&self) -> Vec<i64> {
        self.tracks.iter().map(|t| t.id).collect()
    }

    fn clicked(
        &mut self,
        index: usize,
        track: i64,
        part: RowPart,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        match part {
            RowPart::Row => {
                // The keyboard carries on from the row clicked.
                self.cursor.at = Some(index);
                self.cursor.anchor = Some(index);
                let tracks = &self.tracks;
                let order = || tracks.iter().map(|t| Playable::Track(t.id)).collect();
                if rows::select(
                    &mut self.selection,
                    Playable::Track(track),
                    event,
                    order,
                    cx,
                ) {
                    let ids = self.tracks.iter().map(|t| t.id).collect();
                    actions::play_list(&self.lib, ids, index, cx);
                }
                cx.notify();
            }
            RowPart::Artist(id) => self
                .lib
                .nav
                .update(cx, |n, cx| n.open(Page::Artist(id), cx)),
            RowPart::Album(id) => self.lib.nav.update(cx, |n, cx| n.open(Page::Album(id), cx)),
            RowPart::Heart => self
                .lib
                .feed
                .update(cx, |f, cx| f.toggle_favourite(Playable::Track(track), cx)),
        }
    }
}

impl Render for AlbumPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(super::covers::UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        let width = self.width.clone();
        let entity = cx.entity_id();
        let measure = crate::components::width_probe(width, entity);
        let content = match (self.width.get(), self.gone) {
            (_, Some(failed)) => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(p.space(Space::M))
                .child(
                    div()
                        .typeset(p, Type::TitleSmall, p.muted)
                        .child(if failed {
                            "couldn't read this album"
                        } else {
                            "this album isn't in the library any more"
                        }),
                )
                .when(failed, |d| {
                    d.child(
                        chrome::text_button("page-retry", "try again")
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                })
                .into_any_element(),
            (Some(width), None) if self.album.is_some() => {
                let grid = Grid::for_width(width, &p.look.scale);
                let lead_width = rows::lead_width(p, self.tracks.len(), window);
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(self.header(p, grid, window, cx))
                    .child(actions::list_menu(
                        div()
                            .id("album-track-list")
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                uniform_list(
                                    "album-tracks",
                                    self.lines.len(),
                                    cx.processor(move |this, range: Range<usize>, window, cx| {
                                        let p = Paint::of(window, cx);
                                        range
                                            .map(|i| this.line(&p, i, lead_width, cx))
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .track_scroll(&self.scroll)
                                .pb(grid.margin)
                                .size_full(),
                            )
                            // GPUI's scrollbar over a virtualised list, which koan-kit's ScrollArea cannot wrap.
                            .child(crate::components::list_scrollbar(&self.scroll)),
                        cx.weak_entity(),
                        window,
                        cx,
                    ))
                    .into_any_element()
            }
            // Still reading: nothing at first, since a local record lands
            // within a frame or two; the spinner once it has not.
            _ if self.slow => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(crate::components::Spinner::new(
                    p.look.font_size(crate::theme::TypeRole::Body),
                ))
                .into_any_element(),
            _ => div().into_any_element(),
        };
        self.keyed = self.focus.is_focused(window);
        super::keys::keyed(
            div().key_context("Library List").track_focus(&self.focus),
            cx,
        )
        .on_action(cx.listener(Self::select_all))
        .on_action(cx.listener(Self::dismiss))
        .relative()
        .size_full()
        .typeset(p, Type::Body, p.ink)
        .child(measure)
        .child(content)
    }
}

impl Focusable for AlbumPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl super::keys::KeyedList for AlbumPage {
    fn cursor(&mut self) -> &mut crate::components::ListCursor {
        &mut self.cursor
    }

    fn len(&self) -> usize {
        self.tracks.len()
    }

    fn pick(&mut self, from: usize, to: usize, _: &mut Context<Self>) {
        let order: Vec<Playable> = self.tracks.iter().map(|t| Playable::Track(t.id)).collect();
        self.selection.only(order[from]);
        if to > from {
            self.selection.click(order[to], true, &order);
        }
    }

    fn toggle(&mut self, ix: usize, _: &mut Context<Self>) {
        let order: Vec<Playable> = self.tracks.iter().map(|t| Playable::Track(t.id)).collect();
        self.selection.click(order[ix], false, &order);
    }

    /// The line the track is drawn on, past the disc headings above it.
    fn reveal(&mut self, ix: usize, _: &mut Context<Self>) {
        let line = self
            .lines
            .iter()
            .position(|l| matches!(l, Line::Track { index, .. } if *index == ix));
        if let Some(line) = line {
            self.scroll
                .scroll_to_item(line, crate::components::ScrollStrategy::Top);
        }
    }

    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.tracks.iter().map(|t| t.id).collect();
        actions::play_list(&self.lib, ids, ix, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn track(disc: Option<i32>, number: Option<i32>) -> Track {
        Track {
            id: 0,
            title: String::new(),
            artist_name: String::new(),
            artist_credit: Vec::new(),
            album_artist_name: String::new(),
            album_artist_id: None,
            album_artist_credit: Vec::new(),
            credit_differs: false,
            album_title: String::new(),
            album_id: None,
            artist_id: None,
            disc,
            track_number: number,
            duration_ms: None,
            codec: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "local".into(),
            on_server: false,
            on_disk: true,
            path: None,
            is_favourite: false,
        }
    }

    fn shown(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| match l {
                Line::Disc(t) => format!("[{t}]"),
                Line::Track { number, .. } => number.to_string(),
            })
            .collect()
    }

    #[test]
    fn a_single_disc_numbers_by_place() {
        let tracks = [track(Some(1), Some(5)), track(Some(1), Some(6))];
        assert_eq!(shown(&lines(&tracks)), ["1", "2"]);
    }

    #[test]
    fn discs_get_headings_and_their_own_numbers() {
        let tracks = [
            track(Some(1), Some(1)),
            track(Some(1), None),
            track(Some(2), Some(1)),
            track(None, None),
        ];
        assert_eq!(
            shown(&lines(&tracks)),
            ["[disc 1]", "1", "2", "[disc 2]", "1", "[other tracks]", "1"]
        );
    }
}

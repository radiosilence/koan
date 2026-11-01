//! A record's page: the header with its cover, title, credits as links and
//! play, the one prominent action; then its tracks, under a heading per disc
//! where the record numbers by disc.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Album, Track};

use super::covers::{Art, Covers, Key, Priority, Sleeve};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::rows::{self, RowPart, RowState, TrackList};
use super::selection::Selection;
use super::tile::Grid;
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};
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

pub struct AlbumPage {
    lib: Lib,
    album_id: i64,
    album: Option<Album>,
    tracks: Vec<Track>,
    lines: Vec<Line>,
    /// Whether any row names its artists, which makes every line taller.
    credited: bool,
    /// Read and found nothing: gone from the library, or the read failed.
    gone: Option<bool>,
    request: u64,
    loaded_version: Option<u64>,
    _load: Option<Task<()>>,
    scroll: UniformListScrollHandle,
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
            cx.on_release(move |this, cx| this.lib.covers.update(cx, |c, _| c.release(id))),
        ];
        let mut this = Self {
            lib,
            album_id,
            album: None,
            tracks: Vec::new(),
            lines: Vec::new(),
            credited: false,
            gone: None,
            request: 0,
            loaded_version: None,
            _load: None,
            scroll: UniformListScrollHandle::new(),
            width: Rc::new(Cell::new(None)),
            selection: Selection::default(),
            pick: None,
            menu: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.load(cx);
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
                        this.credited = page.tracks.iter().any(|t| t.credit_differs);
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
            self.pick = Some((revision, PlayableDrag::new(picked, label)));
        }
        self.pick.as_ref().map(|(_, drag)| drag.clone())
    }

    /// One height for every line: taller when any row names its artists.
    fn row_height(&self, p: &Paint) -> Pixels {
        if self.credited {
            rows::text_height(p)
        } else {
            rows::height(p)
        }
    }

    fn line(
        &mut self,
        p: &Paint,
        i: usize,
        lead_width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let height = self.row_height(p);
        let (index, number) = match &self.lines[i] {
            Line::Disc(title) => {
                return div()
                    .w_full()
                    .h(height)
                    .flex()
                    .items_end()
                    .px(px(super::tile::MARGIN))
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
            _ => PlayableDrag::new(&[item], title),
        };
        let feed = self.lib.feed.read(cx);
        let state = RowState {
            lead: number,
            lead_width,
            current: feed.playing_track == Some(id),
            playing: feed.playing,
            favourite: feed.is_favourite(item),
            selected,
            gathered: false,
            transfer: feed.transfers.get(&id).copied(),
            drag,
            height,
        };
        rows::track_row(p, &self.tracks[index], index, state, None, None, cx)
    }

    fn header(
        &mut self,
        p: &Paint,
        grid: Grid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let album = self.album.clone();
        let key = Key::new(Art::Album(self.album_id), grid.side, window.scale_factor());
        let owner = cx.entity_id();
        let sleeve = self.lib.covers.update(cx, |c: &mut Covers, _| {
            c.request(owner, [(key, Priority::Visible)]);
            c.sleeve(key)
        });
        let mut cover = div().size(grid.side).flex_none().bg(p.surface);
        match sleeve {
            Sleeve::Ready(image) => {
                cover = cover.child(img(ImageSource::Render(image)).size_full())
            }
            Sleeve::Colour(colour) => cover = cover.bg(colour),
            Sleeve::Waiting | Sleeve::None => {}
        }
        let total: i64 = self.tracks.iter().filter_map(|t| t.duration_ms).sum();
        let favourite = self
            .lib
            .feed
            .read(cx)
            .is_favourite(Playable::Album(self.album_id));
        let item = Playable::Album(self.album_id);
        let mut details = div().flex().flex_wrap().typeset(p, Type::Control, p.muted);
        if let Some(album) = &album {
            for (i, part) in album.artist_credit.iter().enumerate() {
                let span = div().id(("page-credit", i)).child(part.text.clone());
                details = details.child(match part.artist_id {
                    Some(artist) => {
                        let nav = self.lib.nav.clone();
                        span.cursor_pointer()
                            .text_color(p.ink)
                            .hover(|s| s.underline())
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
        let lib = self.lib.clone();
        let lib2 = self.lib.clone();
        let lib3 = self.lib.clone();
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(grid.gap)
            .px(grid.margin)
            .pt(p.space(Space::L))
            .pb(p.space(Space::L))
            .child(cover)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(p.space(Space::S))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeset(p, Type::Title, p.strong)
                            .line_clamp(2)
                            .child(album.as_ref().map(|a| a.title.clone()).unwrap_or_default()),
                    )
                    .child(details)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(p.space(Space::L))
                            .pt(p.space(Space::S))
                            .child(
                                // The page's one prominent action.
                                crate::components::Button::new(
                                    "page-play",
                                    crate::components::ButtonKind::Prominent,
                                )
                                .label("play")
                                .on_click(move |_, _, cx| {
                                    actions::play(&lib, &[item], false, cx).detach()
                                }),
                            )
                            .child(chrome::text_button("page-shuffle", "shuffle").on_click(
                                move |_, _, cx| actions::play(&lib2, &[item], true, cx).detach(),
                            ))
                            .child(chrome::text_button("page-queue", "add to queue").on_click(
                                move |_, _, cx| {
                                    actions::queue(&lib3, &[item], koan_ffi::Placement::Append, cx)
                                        .detach()
                                },
                            ))
                            .child(
                                div()
                                    .id("page-heart")
                                    .cursor_pointer()
                                    .typeset(
                                        p,
                                        Type::Meta,
                                        if favourite { p.accent } else { p.muted },
                                    )
                                    .child(super::tile::heart_glyph(p, favourite))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.lib
                                            .feed
                                            .update(cx, |f, cx| f.toggle_favourite(item, cx))
                                    })),
                            ),
                    ),
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
        let measure = canvas(
            move |bounds, _, cx| {
                if width.get() != Some(bounds.size.width) {
                    width.set(Some(bounds.size.width));
                    cx.notify(entity);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
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
                            .child(gpui_kit::base::Scrollbar::vertical(&self.scroll)),
                        cx.weak_entity(),
                        window,
                        cx,
                    ))
                    .into_any_element()
            }
            // Still reading: nothing rather than a spinner, since a local
            // record lands within a frame or two.
            _ => div().into_any_element(),
        };
        div()
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::dismiss))
            .relative()
            .size_full()
            .typeset(p, Type::Body, p.ink)
            .child(measure)
            .child(content)
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

//! An artist's page: the photograph and biography from their Wikipedia
//! article, play as the one prominent action, their records under a heading
//! per kind of release, then the records they feature on. An artist with no
//! records of their own shows the tracks they appear on instead.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Album, AlbumSort, Artist, ArtistInfo, BrowseFilter, ReleaseSection, Track};

use super::covers::{Art, Covers, Key, Priority, Sleeve};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::rows::{self, RowPart, RowState, TrackList};
use super::selection::Selection;
use super::tile::{self, Grid, Part, TileHost, TileState};
use super::tokens::{self, Type, Typeset, space};
use super::{Dismiss, Lib, SelectAll, actions, chrome};

struct Record {
    artist: Option<Artist>,
    albums: Vec<Album>,
    features: Vec<Album>,
    appearances: Vec<Track>,
}

pub struct ArtistPage {
    lib: Lib,
    artist_id: i64,
    record: Option<Record>,
    info: Option<ArtistInfo>,
    /// The biography shown whole rather than its opening.
    expanded: bool,
    loaded_version: Option<u64>,
    request: u64,
    _load: Option<Task<()>>,
    _enrich: Option<Task<()>>,
    width: Rc<Cell<Option<Pixels>>>,
    scroll: ScrollHandle,
    selection: Selection,
    pick: Option<(u64, PlayableDrag)>,
    starting: HashSet<i64>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ArtistPage {
    pub fn new(lib: Lib, artist_id: i64, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        let subscriptions = vec![
            cx.observe(&lib.feed, |this, _, cx| {
                this.load(cx);
                cx.notify();
            }),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.subscribe(&lib.covers, |this, _, gone: &super::covers::Evicted, cx| {
                let albums: HashSet<i64> = gone.albums.iter().copied().collect();
                let tracks: HashSet<i64> = gone.tracks.iter().copied().collect();
                this.selection.retain(|p| match p {
                    Playable::Album(id) => !albums.contains(&id),
                    Playable::Track(id) => !tracks.contains(&id),
                    Playable::Artist(_) => true,
                });
                cx.notify();
            }),
            cx.on_release(move |this, cx| {
                let offset = -this.scroll.offset().y;
                let page = Page::Artist(this.artist_id);
                this.lib
                    .nav
                    .update(cx, |nav, _| nav.remember_offset(page, offset));
                this.lib.covers.update(cx, |c, _| c.release(id));
            }),
        ];
        let scroll = ScrollHandle::new();
        if let Some(y) = lib.nav.read(cx).offset(Page::Artist(artist_id)) {
            scroll.set_offset(point(px(0.0), -y));
        }
        let mut this = Self {
            lib,
            artist_id,
            record: None,
            info: None,
            expanded: false,
            loaded_version: None,
            request: 0,
            _load: None,
            _enrich: None,
            width: Rc::new(Cell::new(None)),
            scroll,
            selection: Selection::default(),
            pick: None,
            starting: HashSet::new(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this.enrich(cx);
        this
    }

    /// Everything the page draws, read together and again when the library
    /// moves. The biography comes from the cache here; `enrich` asks further.
    fn load(&mut self, cx: &mut Context<Self>) {
        let version = self.lib.feed.read(cx).library_version;
        if version.is_none() || version == self.loaded_version {
            return;
        }
        self.loaded_version = version;
        self.request += 1;
        let request = self.request;
        let (engine, id) = (self.lib.engine.clone(), self.artist_id);
        self._load = Some(cx.spawn(async move |this, cx| {
            let artist = engine.clone().artist(id).await.ok().flatten();
            let albums = engine
                .clone()
                .albums(Some(id), AlbumSort::Year, 0, None, BrowseFilter::default())
                .await
                .unwrap_or_default();
            let features = engine.clone().artist_features(id).await.unwrap_or_default();
            let info = engine.clone().artist_info(id).await.ok().flatten();
            // Only when there is nothing else to show: an artist with records
            // has their tracks one click away on each.
            let appearances = if albums.is_empty() {
                engine.tracks(None, Some(id)).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                this.record = Some(Record {
                    artist,
                    albums,
                    features,
                    appearances,
                });
                if this.info.is_none() {
                    this.info = info;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The biography and photograph, fetched when the cache has none or a
    /// stale one: seconds on a miss, never waited on.
    fn enrich(&mut self, cx: &mut Context<Self>) {
        let (engine, id) = (self.lib.engine.clone(), self.artist_id);
        self._enrich = Some(cx.spawn(async move |this, cx| {
            let fetched = engine.fetch_artist_info(id).await.ok().flatten();
            this.update(cx, |this, cx| {
                if fetched.is_some() {
                    this.info = fetched;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn pick(&mut self) -> Option<PlayableDrag> {
        if !self.selection.is_active() || self.selection.picked().is_empty() {
            return None;
        }
        let revision = self.selection.revision();
        if self.pick.as_ref().is_none_or(|(r, _)| *r != revision) {
            let picked = self.selection.picked();
            let label = chrome::count(picked.len() as u64, "item", "items");
            self.pick = Some((revision, PlayableDrag::new(picked, label)));
        }
        self.pick.as_ref().map(|(_, drag)| drag.clone())
    }

    fn order(&self) -> Vec<Playable> {
        let Some(r) = &self.record else {
            return Vec::new();
        };
        r.albums
            .iter()
            .chain(&r.features)
            .map(|a| Playable::Album(a.id))
            .chain(r.appearances.iter().map(|t| Playable::Track(t.id)))
            .collect()
    }

    fn tiles(
        &mut self,
        albums: &[Album],
        show_artist: bool,
        grid: Grid,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let pick = self.pick();
        let entity = cx.entity();
        let after: actions::After = Some(Rc::new(move |cx: &mut App| {
            entity.update(cx, |this, cx| {
                this.selection.end();
                cx.notify();
            })
        }));
        let mut tiles = Vec::with_capacity(albums.len());
        let mut keys = Vec::with_capacity(albums.len());
        for album in albums {
            let item = Playable::Album(album.id);
            let selected = self.selection.contains(item);
            let (drag, after) = match (&pick, selected) {
                (Some(pick), true) => (pick.clone(), after.clone()),
                _ => (PlayableDrag::new(&[item], album.title.clone()), None),
            };
            let feed = self.lib.feed.read(cx);
            let state = TileState {
                favourite: feed.is_favourite(item),
                playing: feed.playing_album == Some(album.id),
                arriving: feed.arriving.contains_key(&album.id),
                selecting: self.selection.is_active(),
                selected,
                starting: self.starting.contains(&album.id),
                show_artist,
                drag,
                after,
            };
            let key = Key::new(Art::Album(album.id), grid.side, scale);
            keys.push(key);
            let sleeve = self
                .lib
                .covers
                .update(cx, |c: &mut Covers, _| c.sleeve(key));
            tiles.push(tile::tile(album, grid.side, sleeve, state, self, cx));
        }
        div()
            .flex()
            .flex_wrap()
            .gap_x(grid.gap)
            .gap_y(px(22.0))
            .children(tiles)
    }

    fn heading(title: impl Into<SharedString>) -> Div {
        div()
            .pt(space::s16())
            .pb(space::s8())
            .typeset(Type::Fine, tokens::INK)
            .child(title.into())
    }

    fn header(&mut self, grid: Grid, scale: f32, cx: &mut Context<Self>) -> Div {
        let id = self.artist_id;
        let item = Playable::Artist(id);
        let name = self
            .record
            .as_ref()
            .and_then(|r| r.artist.as_ref())
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let mut photo = div().size(grid.side).flex_none().bg(tokens::SURFACE);
        if self.info.as_ref().is_some_and(|i| i.has_image) {
            let key = Key::new(Art::Artist(id), grid.side, scale);
            if let Sleeve::Ready(image) = self.lib.covers.update(cx, |c, _| c.sleeve(key)) {
                photo = photo.child(img(ImageSource::Render(image)).size_full());
            }
        }
        let favourite = self.lib.feed.read(cx).is_favourite(item);
        let (lib, lib2) = (self.lib.clone(), self.lib.clone());
        let mut text =
            div()
                .flex()
                .flex_col()
                .gap(space::s8())
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .typeset(Type::Title, tokens::STRONG)
                        .line_clamp(2)
                        .child(name),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::s16())
                        .child(
                            div()
                                .id("artist-play")
                                .cursor_pointer()
                                .px(px(20.0))
                                .py(px(12.0))
                                .border_1()
                                .border_color(tokens::ACCENT)
                                .typeset(Type::Body, tokens::ACCENT)
                                .active(|s| s.bg(tokens::HOVER))
                                .child("play")
                                .on_click(move |_, _, cx| {
                                    actions::play(&lib, &[item], false, cx).detach()
                                }),
                        )
                        .child(chrome::text_button("artist-shuffle", "shuffle").on_click(
                            move |_, _, cx| actions::play(&lib2, &[item], true, cx).detach(),
                        ))
                        .child(
                            div()
                                .id("artist-page-heart")
                                .cursor_pointer()
                                .typeset(
                                    Type::Meta,
                                    if favourite {
                                        tokens::ACCENT
                                    } else {
                                        tokens::MUTED
                                    },
                                )
                                .child(if favourite { "♥" } else { "♡" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.lib
                                        .feed
                                        .update(cx, |f, cx| f.toggle_favourite(item, cx))
                                })),
                        ),
                );
        if let Some(info) = &self.info
            && let Some(bio) = &info.bio
        {
            let mut body = div()
                .typeset(Type::Control, tokens::INK)
                .child(bio.replace('\n', "\n\n"));
            if !self.expanded {
                body = body.line_clamp(4);
            }
            text = text.child(body).child(
                div()
                    .flex()
                    .gap(space::s12())
                    .typeset(Type::Fine, tokens::MUTED)
                    .when(!self.expanded, |d| {
                        d.child(
                            div()
                                .id("bio-more")
                                .cursor_pointer()
                                .underline()
                                .child("more")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.expanded = true;
                                    cx.notify();
                                })),
                        )
                    })
                    .when_some(info.bio_url.clone(), |d, url| {
                        d.child(
                            div()
                                .id("bio-source")
                                .cursor_pointer()
                                .underline()
                                .child("wikipedia")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
                    .when_some(info.image_credit.clone(), |d, credit| {
                        d.child(format!("photo: {credit}"))
                    }),
            );
        }
        div()
            .flex()
            .items_start()
            .gap(grid.gap)
            .pt(px(16.0))
            .pb(space::s16())
            .child(photo)
            .child(text)
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let order = self.order();
        self.selection.select_all(&order);
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_active() {
            self.selection.end();
            cx.notify();
        }
    }
}

/// The sections of an artist's records, in page order: the engine lists them
/// section by section, so a section is a run of them.
fn sections(albums: &[Album]) -> Vec<(ReleaseSection, &[Album])> {
    let mut out: Vec<(ReleaseSection, &[Album])> = Vec::new();
    let mut start = 0;
    for i in 1..=albums.len() {
        if i == albums.len() || albums[i].release_section != albums[start].release_section {
            out.push((albums[start].release_section, &albums[start..i]));
            start = i;
        }
    }
    out
}

impl TileHost for ArtistPage {
    fn lib(&self) -> &Lib {
        &self.lib
    }

    fn clicked(
        &mut self,
        album: i64,
        part: Part,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let item = Playable::Album(album);
        let order = self.order();
        if self
            .selection
            .take(item, modifiers.shift, modifiers.secondary(cx), &order)
        {
            cx.notify();
            return;
        }
        match part {
            Part::Sleeve => {
                self.starting.insert(album);
                cx.notify();
                let started = actions::play(&self.lib, &[item], false, cx);
                self.lib
                    .nav
                    .update(cx, |nav, cx| nav.open(Page::Album(album), cx));
                cx.spawn(async move |this, cx| {
                    started.await;
                    this.update(cx, |this, cx| {
                        this.starting.remove(&album);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            Part::Title => self
                .lib
                .nav
                .update(cx, |nav, cx| nav.open(Page::Album(album), cx)),
            Part::Artist(artist) => self
                .lib
                .nav
                .update(cx, |nav, cx| nav.open(Page::Artist(artist), cx)),
            Part::Heart => self
                .lib
                .feed
                .update(cx, |f, cx| f.toggle_favourite(item, cx)),
        }
    }
}

impl TrackList for ArtistPage {
    fn lib(&self) -> &Lib {
        &self.lib
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
                let ids: Vec<i64> = self
                    .record
                    .as_ref()
                    .map(|r| r.appearances.iter().map(|t| t.id).collect())
                    .unwrap_or_default();
                let order = || ids.iter().map(|id| Playable::Track(*id)).collect();
                if rows::select(
                    &mut self.selection,
                    Playable::Track(track),
                    event,
                    order,
                    cx,
                ) {
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

impl chrome::Picking for ArtistPage {
    fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }
}

impl Render for ArtistPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let mut content = div()
            .flex()
            .flex_col()
            .px(px(tile::MARGIN))
            .pb(px(tile::MARGIN));
        if let (Some(width), Some(record)) = (self.width.get(), self.record.take()) {
            let grid = Grid::for_width(width);
            let scale = window.scale_factor();
            content = content.child(self.header(grid, scale, cx));
            let sections = sections(&record.albums);
            let titled = sections.len() > 1;
            for (section, albums) in sections {
                if titled {
                    content = content.child(Self::heading(
                        koan_ffi::release_section_title(section).to_lowercase(),
                    ));
                }
                content = content.child(self.tiles(albums, false, grid, scale, cx));
            }
            if !record.features.is_empty() {
                let items: Vec<Playable> = record
                    .features
                    .iter()
                    .map(|a| Playable::Album(a.id))
                    .collect();
                let (lib, items2, lib2) = (self.lib.clone(), items.clone(), self.lib.clone());
                content = content
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(space::s16())
                            .child(Self::heading("features on"))
                            .child(
                                chrome::text_button("features-play", "play")
                                    .pb(space::s8())
                                    .on_click(move |_, _, cx| {
                                        actions::play(&lib, &items, false, cx).detach()
                                    }),
                            )
                            .child(
                                chrome::text_button("features-shuffle", "shuffle")
                                    .pb(space::s8())
                                    .on_click(move |_, _, cx| {
                                        actions::play(&lib2, &items2, true, cx).detach()
                                    }),
                            ),
                    )
                    .child(self.tiles(&record.features, true, grid, scale, cx));
            }
            if !record.appearances.is_empty() {
                content = content.child(Self::heading("appears on"));
                let lead_width = rows::lead_width(record.appearances.len(), window);
                let pick = self.pick();
                for (index, track) in record.appearances.iter().enumerate() {
                    let item = Playable::Track(track.id);
                    let selected = self.selection.contains(item);
                    let drag = match (selected, &pick) {
                        (true, Some(pick)) => pick.clone(),
                        _ => PlayableDrag::new(&[item], track.title.clone()),
                    };
                    let feed = self.lib.feed.read(cx);
                    let state = RowState {
                        lead: (index + 1).to_string().into(),
                        lead_width,
                        current: feed.playing_track == Some(track.id),
                        playing: feed.playing,
                        favourite: feed.is_favourite(item),
                        selected,
                        gathered: true,
                        transfer: feed.transfers.get(&track.id).copied(),
                        drag,
                        height: rows::art_height(),
                    };
                    let sleeve = track.album_id.map(|album| {
                        let key = Key::new(Art::Album(album), px(rows::SLEEVE), scale);
                        self.lib.covers.update(cx, |c, _| c.sleeve(key))
                    });
                    content = content.child(rows::track_row(
                        track,
                        index,
                        state,
                        sleeve,
                        Some(self.artist_id),
                        self,
                        cx,
                    ));
                }
            }
            // Every sleeve the page draws, wanted together: a page holds at
            // most a few hundred, and the visible ones lead the queue anyway.
            let mut wanted: Vec<(Key, Priority)> = record
                .albums
                .iter()
                .chain(&record.features)
                .map(|a| {
                    (
                        Key::new(Art::Album(a.id), grid.side, scale),
                        Priority::Ahead(0.0),
                    )
                })
                .chain(
                    record
                        .appearances
                        .iter()
                        .filter_map(|t| t.album_id)
                        .map(|a| {
                            (
                                Key::new(Art::Album(a), px(rows::SLEEVE), scale),
                                Priority::Ahead(0.0),
                            )
                        }),
                )
                .collect();
            if self.info.as_ref().is_some_and(|i| i.has_image) {
                wanted.push((
                    Key::new(Art::Artist(self.artist_id), grid.side, scale),
                    Priority::Visible,
                ));
            }
            let owner = cx.entity_id();
            self.lib.covers.update(cx, |c, _| c.request(owner, wanted));
            self.record = Some(record);
        }

        div()
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::dismiss))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(tokens::BG)
            .typeset(Type::Body, tokens::INK)
            .child(measure)
            .child(
                div()
                    .id("artist-page")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(content),
            )
            .when(self.selection.is_active(), |d| {
                d.child(chrome::selection_bar(
                    &self.lib,
                    &self.selection,
                    cx.entity(),
                    cx,
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn album(id: i64, section: ReleaseSection) -> Album {
        Album {
            id,
            title: String::new(),
            artist_id: 1,
            artist_name: String::new(),
            artist_credit: Vec::new(),
            year: None,
            codec: None,
            label: None,
            total_discs: None,
            total_tracks: None,
            added_at: None,
            on_device: None,
            release_badge: None,
            release_section: section,
        }
    }

    #[test]
    fn sections_are_runs_of_the_engines_order() {
        let albums = [
            album(1, ReleaseSection::Albums),
            album(2, ReleaseSection::Albums),
            album(3, ReleaseSection::Live),
        ];
        let runs = sections(&albums);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].0, ReleaseSection::Albums);
        assert_eq!(runs[0].1.len(), 2);
        assert_eq!(runs[1].1[0].id, 3);
        assert!(sections(&[]).is_empty());
    }
}

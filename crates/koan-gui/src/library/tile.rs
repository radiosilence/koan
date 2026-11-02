//! One record in a grid, shared by the album browser and an artist's page so
//! they behave alike: the sleeve plays the record, the title opens it, each
//! credited artist links to their page. While picking, the whole tile ticks.
//!
//! Each tile is a cached view of its own: the pointer moving over a grid
//! redraws the tile it enters and the one it leaves, and nothing else.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use koan_ffi::{Album, AlbumOnDevice};
use koan_kit::app::*;

use super::actions;
use super::covers::Sleeve;
use super::drag::PlayableDrag;
use super::tokens::{Paint, Type, Typeset};
use crate::components::{TileArt, TileGrid, TileMark, TileSleeve, origin_probe, text_widths};
use crate::theme::tokens::Motion;
use crate::theme::{Icon, KoanIcon, KoanTheme};

use crate::theme::Hsla;

/// The record grid, koan-kit's: every tile the same size, nothing measured.
pub type Grid = TileGrid;

/// Which part of a tile was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Sleeve,
    Title,
    Artist(i64),
    Heart,
}

/// What a page holding tiles does with them.
pub trait TileHost: actions::Menus {
    fn clicked(
        &mut self,
        album: i64,
        part: Part,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    );
}

/// A record as its tile draws it: the strings made once, when the listing
/// arrives, so a frame neither formats nor copies text, and the shaped lines
/// GPUI caches by their text are found again.
pub struct Record {
    pub id: i64,
    pub title: SharedString,
    pub credit: Vec<(SharedString, Option<i64>)>,
    /// " · 1999" after a credit, "1999" alone.
    pub year_after: Option<SharedString>,
    pub year: Option<SharedString>,
    pub codec: Option<SharedString>,
    pub badge: Option<SharedString>,
    pub on_device: Option<AlbumOnDevice>,
}

impl Record {
    pub fn of(album: &Album) -> Self {
        Self {
            id: album.id,
            title: album.title.clone().into(),
            credit: album
                .artist_credit
                .iter()
                .map(|part| (part.text.clone().into(), part.artist_id))
                .collect(),
            year_after: album.year.map(|y| format!(" · {y}").into()),
            year: album.year.map(|y| y.to_string().into()),
            codec: album.codec.as_ref().map(|c| c.to_uppercase().into()),
            badge: album.release_badge.clone().map(Into::into),
            on_device: album.on_device,
        }
    }
}

/// What a tile shows beyond its record. Compared on each frame, so a tile
/// whose state did not move replays its last frame.
#[derive(Clone)]
pub struct TileState {
    pub favourite: bool,
    pub playing: bool,
    pub arriving: bool,
    pub selecting: bool,
    pub selected: bool,
    pub starting: bool,
    pub show_artist: bool,
    pub side: Pixels,
    pub sleeve: Sleeve,
    /// What dragging the tile carries, and what its menu acts on: the page's
    /// pick when the tile is in it, built once for the pick; itself
    /// otherwise.
    pub drag: PlayableDrag,
    /// Which pick `drag` was built for; `None` for the tile alone.
    pub pick: Option<u64>,
    /// Run after a menu verb: ends the pick when the menu acted on one.
    pub after: actions::After,
}

impl TileState {
    fn same(&self, other: &TileState) -> bool {
        self.favourite == other.favourite
            && self.playing == other.playing
            && self.arriving == other.arriving
            && self.selecting == other.selecting
            && self.selected == other.selected
            && self.starting == other.starting
            && self.show_artist == other.show_artist
            && self.side == other.side
            && self.pick == other.pick
            && self.sleeve.same(&other.sleeve)
    }
}

/// One tile as a view of its own, rendered cached by the page: a hover, a
/// heart or a sleeve arriving redraws this tile, and every other tile on
/// screen replays its last frame.
pub struct TileView<V: TileHost> {
    host: WeakEntity<V>,
    record: Rc<Record>,
    state: TileState,
    /// The part under the pointer: the play mark and dim (over the sleeve),
    /// the heart (anywhere on the tile) and the underlines are made only
    /// then, so the tiles on screen carry no hidden overlays.
    hovered: Option<Part>,
    /// Where the tile was laid out, for telling the parts apart by position:
    /// the tile has one interactive element, not one per part, since a
    /// scrolling grid lays out and hit-tests every tile every frame.
    origin: Rc<Cell<Point<Pixels>>>,
    /// The credit's names' widths, shaped at the size they were drawn at
    /// when the pointer first needed them.
    credit_widths: Option<(Pixels, Vec<Pixels>)>,
    /// When the sleeve being faded in arrived, while it fades.
    fade: Option<Instant>,
    /// The record's colour, last shown before its sleeve: what the sleeve
    /// fades in over.
    ground: Option<Hsla>,
    _accent: Subscription,
}

impl<V: TileHost> TileView<V> {
    pub fn new(
        host: WeakEntity<V>,
        record: Rc<Record>,
        state: TileState,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            host,
            record,
            state,
            hovered: None,
            origin: Rc::new(Cell::new(Point::default())),
            credit_widths: None,
            fade: None,
            ground: None,
            _accent: crate::theme::observe_accent(cx),
        }
    }

    /// The part of the tile at `position`, in window coordinates.
    fn part_at(&mut self, position: Point<Pixels>, window: &Window, cx: &App) -> Option<Part> {
        let local = position - self.origin.get();
        let (x, y) = (f32::from(local.x), f32::from(local.y));
        let side = f32::from(self.state.side);
        if x < 0.0 || y < 0.0 || x > side {
            return None;
        }
        if y < side {
            let shown = !self.state.selecting;
            return Some(if shown && TileArt::on_heart(self.state.side, x, y) {
                Part::Heart
            } else {
                Part::Sleeve
            });
        }
        let look = crate::theme::Look::of(window, cx);
        let (title, credit) = TileGrid::caption_lines(&look, self.state.side);
        if title.contains(&y) {
            return Some(Part::Title);
        }
        if !self.state.show_artist || !credit.contains(&y) {
            return None;
        }
        let size = look.font_size(Type::Fine);
        if self
            .credit_widths
            .as_ref()
            .is_none_or(|(at, _)| *at != size)
        {
            let texts: Vec<SharedString> =
                self.record.credit.iter().map(|(t, _)| t.clone()).collect();
            self.credit_widths = Some((size, text_widths(&texts, Type::Fine, window, cx)));
        }
        let widths = &self.credit_widths.as_ref()?.1;
        let mut left = 0.0;
        for ((_, artist), width) in self.record.credit.iter().zip(widths) {
            let right = left + f32::from(*width);
            if (left..right).contains(&x) {
                return artist.map(Part::Artist);
            }
            left = right;
        }
        None
    }

    /// Shows `record` in `state`, redrawing only if either moved.
    pub fn show(&mut self, record: &Rc<Record>, state: TileState, cx: &mut Context<Self>) {
        if !Rc::ptr_eq(&self.record, record) {
            self.credit_widths = None;
        }
        let moved = !Rc::ptr_eq(&self.record, record) || !self.state.same(&state);
        // A sleeve arriving where there was none fades in; one replacing a
        // stand-in of another size, or already in memory, is just shown.
        if let Sleeve::Colour(colour) = &state.sleeve {
            self.ground = Some(*colour);
        }
        self.fade = match (&self.state.sleeve, &state.sleeve) {
            (Sleeve::Ready(..), Sleeve::Ready(..)) => self.fade,
            (_, Sleeve::Ready(_, arrived)) if arrived.elapsed() < Motion::NORMAL => Some(*arrived),
            _ => None,
        };
        self.record = record.clone();
        self.state = state;
        if moved {
            cx.notify();
        }
    }
}

impl<V: TileHost> Render for TileView<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        // A tile scrolled under a still pointer: the part under it is found
        // again here, since no move tells the tile so.
        if self.hovered.is_some() {
            self.hovered = self.part_at(window.mouse_position(), window, cx);
        }
        let (record, state) = (&self.record, &self.state);
        let id = record.id;
        let side = state.side;
        let hovered = self.hovered;

        // Nothing below takes the pointer; the tile's root does, for all of
        // it (see `origin`).
        let sleeve = match &state.sleeve {
            Sleeve::Ready(image, _) => {
                let shown = match self.fade.filter(|_| KoanTheme::motion_allowed(cx)) {
                    Some(arrived) => {
                        let t = arrived.elapsed().as_secs_f32() / Motion::NORMAL.as_secs_f32();
                        if t < 1.0 {
                            // This tile alone, frame by frame, until it lands.
                            cx.on_next_frame(window, |_, _, cx| cx.notify());
                            Motion::quick_out(t)
                        } else {
                            self.fade = None;
                            1.0
                        }
                    }
                    None => 1.0,
                };
                TileSleeve::Image {
                    image: image.clone(),
                    shown,
                    ground: self.ground,
                }
            }
            Sleeve::Colour(colour) => TileSleeve::Colour(*colour),
            Sleeve::Waiting | Sleeve::None => TileSleeve::Missing,
        };
        let over_sleeve = matches!(hovered, Some(Part::Sleeve | Part::Heart));
        let mark = if state.starting {
            Some(TileMark::Starting)
        } else if over_sleeve && !state.selecting {
            Some(TileMark::Play)
        } else {
            None
        };
        let art = TileArt::new(side, sleeve)
            .codec(record.codec.clone())
            .badge(record.badge.clone())
            .mark(mark)
            .playing(
                state
                    .playing
                    .then(|| crate::levels::bars(p.accent).into_any_element()),
            )
            .on_device(on_device(record.on_device, state.arriving))
            .heart(
                (hovered.is_some() || state.favourite)
                    .then_some((state.favourite, hovered == Some(Part::Heart))),
            )
            .pick(state.selecting.then_some(state.selected));

        let title = div()
            .w_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .typeset(p, Type::Meta, p.ink)
            .when(hovered == Some(Part::Title) && !state.selecting, |d| {
                d.underline()
            })
            .child(record.title.clone());

        let mut credit = div()
            .flex()
            .w_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .typeset(p, Type::Fine, p.muted);
        if state.show_artist {
            for (text, artist) in &record.credit {
                let lit =
                    !state.selecting && artist.is_some() && hovered == artist.map(Part::Artist);
                credit = credit.child(
                    div()
                        .when(lit, |d| d.underline().text_color(p.ink))
                        .child(text.clone()),
                );
            }
            credit = credit.children(record.year_after.clone());
        } else {
            credit = credit.children(record.year.clone());
        }

        let origin = self.origin.clone();
        let host = self.host.clone();
        let drag = state.drag.clone();
        let target = actions::MenuTarget {
            items: drag.items.clone(),
            album: None,
            after: state.after.clone(),
        };
        div()
            .id(("tile", id as u64))
            .relative()
            .flex()
            .flex_col()
            .w(side)
            .gap(TileGrid::caption_gap())
            .cursor_pointer()
            .child(origin_probe(origin))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                let part = this.part_at(e.position, window, cx);
                if part != this.hovered {
                    this.hovered = part;
                    cx.notify();
                }
            }))
            // Entering or leaving, by the pointer or by a scroll under a still
            // one: GPUI checks hover again as it paints.
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                let part = if *hovered {
                    this.part_at(window.mouse_position(), window, cx)
                } else {
                    None
                };
                if part != this.hovered {
                    this.hovered = part;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                // While picking, or starting a pick with ⌘, any part ticks the
                // tile, the gaps between its lines included: the host takes it.
                let picks = this.state.selecting || e.modifiers().secondary(cx);
                let part = this
                    .part_at(e.position(), window, cx)
                    .or(picks.then_some(Part::Sleeve));
                let Some(part) = part else { return };
                cx.stop_propagation();
                let modifiers = e.modifiers();
                host.update(cx, |host, cx| host.clicked(id, part, modifiers, window, cx))
                    .ok();
            }))
            .child(art)
            .child(title)
            .child(credit)
            .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
            .on_mouse_down(MouseButton::Right, actions::mark(self.host.clone(), target))
    }
}

/// The tile views a page keeps for the records on screen: made when a
/// record scrolls in, kept while it stays, and let go once it has been off
/// screen for a frame.
pub struct Tiles<V: TileHost> {
    views: HashMap<i64, (Entity<TileView<V>>, u64)>,
    frame: u64,
}

impl<V: TileHost> Default for Tiles<V> {
    fn default() -> Self {
        Self {
            views: HashMap::new(),
            frame: 0,
        }
    }
}

impl<V: TileHost> Tiles<V> {
    /// Starts a frame's tiles; the last frame's that were not shown again go.
    pub fn begin(&mut self) {
        let frame = self.frame;
        self.views.retain(|_, (_, used)| *used + 1 >= frame);
        self.frame += 1;
    }

    /// The tile for `record` in `state`, `height` tall with its caption, as
    /// an element the page lays out.
    pub fn tile(
        &mut self,
        record: &Rc<Record>,
        state: TileState,
        height: Pixels,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let frame = self.frame;
        let side = state.side;
        let view = match self.views.get_mut(&record.id) {
            Some((view, used)) => {
                *used = frame;
                view.update(cx, |tile, cx| tile.show(record, state, cx));
                view.clone()
            }
            None => {
                let host = cx.weak_entity();
                let record = record.clone();
                let view = cx.new(|cx| TileView::new(host, record, state, cx));
                self.views
                    .insert(view.read(cx).record.id, (view.clone(), frame));
                view
            }
        };
        AnyView::from(view)
            .cached(StyleRefinement::default().w(side).h(height))
            .into_any_element()
    }
}

/// The heart, filled in the accent for a favourite.
pub fn heart_glyph(p: &Paint, favourite: bool) -> KoanIcon {
    if favourite {
        KoanIcon::new(Icon::Favourited)
            .role(Type::Meta)
            .color(p.accent)
    } else {
        KoanIcon::new(Icon::Favourite).role(Type::Meta).color(p.ink)
    }
}

/// How much of the record is on this device, where the listing says, while
/// it is worth showing: part of it, or more arriving.
fn on_device(on_device: Option<AlbumOnDevice>, arriving: bool) -> Option<f32> {
    match on_device {
        Some(d) if d.total > 0 && (d.have < d.total || arriving) => {
            Some(d.have as f32 / d.total as f32)
        }
        None if arriving => Some(0.0),
        _ => None,
    }
}

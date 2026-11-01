//! One record in a grid, shared by the album browser and an artist's page so
//! they behave alike: the sleeve plays the record, the title opens it, each
//! credited artist links to their page. While picking, the whole tile ticks.
//!
//! Each tile is a cached view of its own: the pointer moving over a grid
//! redraws the tile it enters and the one it leaves, and nothing else.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Album, AlbumOnDevice};

use super::actions;
use super::covers::Sleeve;
use super::drag::PlayableDrag;
use super::tokens::{Paint, Type, Typeset};
use crate::theme::tokens::Scale;
use crate::theme::{Icon, KoanIcon};

/// Columns of at least 150 points sharing the width, up to 210 each, as the
/// Mac's grid lays them out. Every tile is the same size, so nothing is
/// measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub columns: usize,
    pub side: Pixels,
    /// Between tiles in a row: the spare width shared out, at least 18.
    pub gap: Pixels,
    pub margin: Pixels,
    /// A row's height, its spacing below included.
    pub row: Pixels,
}

const MIN_SIDE: f32 = 150.0;
const MAX_SIDE: f32 = 210.0;
const MIN_GAP: f32 = 18.0;
pub const LINE_GAP: f32 = 22.0;
pub const MARGIN: f32 = 20.0;
/// Between sleeve, title and credit.
const CAPTION_GAP: f32 = 7.0;

impl Grid {
    /// The grid for a list `width` wide, with captions on `scale`.
    pub fn for_width(width: Pixels, scale: &Scale) -> Self {
        let usable = (f32::from(width) - 2.0 * MARGIN).max(MIN_SIDE);
        let columns = (((usable + MIN_GAP) / (MIN_SIDE + MIN_GAP)).floor() as usize).max(1);
        let side = ((usable - MIN_GAP * (columns as f32 - 1.0)) / columns as f32)
            .floor()
            .min(MAX_SIDE);
        let gap = if columns > 1 {
            (usable - side * columns as f32) / (columns as f32 - 1.0)
        } else {
            0.0
        };
        Self {
            columns,
            side: px(side),
            gap: px(gap),
            margin: px(MARGIN),
            row: px(side) + caption_height(scale) + px(LINE_GAP),
        }
    }

    pub fn rows(&self, items: usize) -> usize {
        items.div_ceil(self.columns)
    }
}

pub fn caption_height(scale: &Scale) -> Pixels {
    let line = |role: Type| role.points() * scale.text * role.line_height();
    px(CAPTION_GAP + line(Type::Meta) + CAPTION_GAP + line(Type::Fine))
}

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
    /// Under the pointer: the play mark, the dim and the heart are made only
    /// then, so the tiles on screen carry no hidden overlays.
    hovered: bool,
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
            hovered: false,
            _accent: crate::theme::observe_accent(cx),
        }
    }

    /// Shows `record` in `state`, redrawing only if either moved.
    pub fn show(&mut self, record: &Rc<Record>, state: TileState, cx: &mut Context<Self>) {
        let moved = !Rc::ptr_eq(&self.record, record) || !self.state.same(&state);
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
        let (record, state) = (&self.record, &self.state);
        let id = record.id;
        let side = state.side;
        let host = self.host.clone();
        let click = |part: Part| {
            super::listen(
                host.clone(),
                move |this: &mut V, e: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.clicked(id, part, e.modifiers(), window, cx)
                },
            )
        };

        let mut art = div()
            .id(("sleeve", id as u64))
            .group("sleeve")
            .relative()
            .size(side)
            .flex_none()
            .bg(p.surface)
            .cursor_pointer()
            .on_click(click(Part::Sleeve));
        match &state.sleeve {
            Sleeve::Ready(image) => {
                art = art.child(img(ImageSource::Render(image.clone())).size_full())
            }
            Sleeve::Colour(colour) => art = art.bg(*colour),
            Sleeve::Waiting | Sleeve::None => {}
        }
        if let Some(codec) = &record.codec {
            // The codec only: at tile size a rate is unreadable, and the codec
            // is what says whether this is the good copy.
            art = art.child(badge(p, codec.clone()).top(px(6.0)).right(px(6.0)));
        }
        if let (Some(release), false) = (&record.badge, state.selecting) {
            art = art.child(badge(p, release.clone()).top(px(6.0)).left(px(6.0)));
        }
        // The dim snaps in and out for now; it takes the theme's 120 ms when
        // the motion tokens land.
        if state.starting || (self.hovered && !state.selecting) {
            art = art.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.dim())
                    .typeset(p, Type::TitleSmall, p.strong)
                    .when(!state.starting, |d| {
                        d.opacity(0.0).group_hover("sleeve", |s| s.opacity(1.0))
                    })
                    .child(if state.starting {
                        div().child("…").into_any_element()
                    } else {
                        KoanIcon::new(Icon::PlayMark)
                            .role(Type::TitleSmall)
                            .color(p.strong)
                            .into_any_element()
                    }),
            );
        }
        if state.playing {
            art = art.child(playing_bars(p).absolute().left(px(8.0)).bottom(px(8.0)));
        }
        if let Some(bar) = download_bar(p, record.on_device, state.arriving) {
            art = art.child(bar);
        }
        if !state.selecting && (self.hovered || state.favourite) {
            let heart = div()
                .id(("heart", id as u64))
                .absolute()
                .right(px(7.0))
                .bottom(px(7.0))
                .size(px(30.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(p.surface)
                .cursor_pointer()
                .on_click(click(Part::Heart))
                .child(heart_glyph(p, state.favourite));
            art = art.child(heart);
        }
        if state.selecting {
            // Over the art rather than a border on it, so ticking a tile
            // neither shrinks its sleeve nor moves it.
            if state.selected {
                art = art.child(div().absolute().inset_0().border_3().border_color(p.accent));
            }
            // The 20-point tick: white over a fill, the accent when picked.
            let tick = if state.selected {
                KoanIcon::new(Icon::Picked).fill(p.accent)
            } else {
                KoanIcon::new(Icon::Unpicked).fill(hsla(0.0, 0.0, 0.0, 0.25))
            };
            art = art.child(
                div()
                    .absolute()
                    .top(px(7.0))
                    .left(px(7.0))
                    .child(tick.points(20.0 / 1.2).color(hsla(0.0, 0.0, 1.0, 1.0))),
            );
        }

        let title = div()
            .id(("title", id as u64))
            .w_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .typeset(p, Type::Meta, p.ink)
            .cursor_pointer()
            .when(!state.selecting, |d| d.hover(|s| s.underline()))
            .on_click(click(Part::Title))
            .child(record.title.clone());

        let mut credit = div()
            .flex()
            .w_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .typeset(p, Type::Fine, p.muted);
        if state.show_artist {
            for (i, (text, artist)) in record.credit.iter().enumerate() {
                let span = div()
                    .id(("credit", (id as u64) << 8 | i as u64))
                    .child(text.clone());
                credit = credit.child(match artist {
                    Some(artist) if !state.selecting => span
                        .cursor_pointer()
                        .hover(|s| s.underline().text_color(p.ink))
                        .on_click(click(Part::Artist(*artist))),
                    _ => span,
                });
            }
            credit = credit.children(record.year_after.clone());
        } else {
            credit = credit.children(record.year.clone());
        }

        let drag = state.drag.clone();
        let target = actions::MenuTarget {
            items: drag.items.clone(),
            album: None,
            after: state.after.clone(),
        };
        div()
            .id(("tile", id as u64))
            .flex()
            .flex_col()
            .w(side)
            .gap(px(CAPTION_GAP))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.hovered != *hovered {
                    this.hovered = *hovered;
                    cx.notify();
                }
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

fn badge(p: &Paint, text: impl Into<SharedString>) -> Div {
    div()
        .absolute()
        .px(px(6.0))
        .py(px(2.0))
        .bg(p.surface)
        .typeset(p, Type::Fine, p.ink)
        .child(text.into())
}

/// Three bars in the accent: the record is the one playing. Still for now;
/// they move with the music once the analyser's levels are wired.
pub fn playing_bars(p: &Paint) -> Div {
    div()
        .flex()
        .items_end()
        .gap(px(2.0))
        .h(px(12.0))
        .children([7.0, 12.0, 9.0].map(|h| div().w(px(3.0)).h(px(h)).bg(p.accent)))
}

/// How much of the record is on this device, where the listing says, and
/// whether more of it is arriving.
fn download_bar(p: &Paint, on_device: Option<AlbumOnDevice>, arriving: bool) -> Option<Div> {
    let fraction = match on_device {
        Some(d) if d.total > 0 && (d.have < d.total || arriving) => d.have as f32 / d.total as f32,
        None if arriving => 0.0,
        _ => return None,
    };
    Some(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(3.0))
            .bg(p.rule)
            .child(div().h_full().w(relative(fraction)).bg(p.accent)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` brings GPUI's own `test` attribute in with it.
    use core::prelude::v1::test;

    #[test]
    fn columns_share_the_width_within_the_tile_bounds() {
        let narrow = Grid::for_width(px(300.0), &Scale::of(100));
        assert_eq!(narrow.columns, 1);
        assert!(narrow.side <= px(MAX_SIDE));
        let wide = Grid::for_width(px(1920.0), &Scale::of(100));
        assert!(wide.side >= px(MIN_SIDE) && wide.side <= px(MAX_SIDE));
        assert!(wide.gap >= px(MIN_GAP));
        let used = wide.side * wide.columns as f32 + wide.gap * (wide.columns as f32 - 1.0);
        assert!((f32::from(used) - (1920.0 - 2.0 * MARGIN)).abs() < 1.0);
        assert_eq!(wide.rows(0), 0);
        assert_eq!(wide.rows(wide.columns + 1), 2);
    }
}

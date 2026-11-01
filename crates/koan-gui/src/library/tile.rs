//! One record in a grid, shared by the album browser and an artist's page so
//! they behave alike: the sleeve plays the record, the title opens it, each
//! credited artist links to their page. While picking, the whole tile ticks.
//!
//! Each tile is a cached view of its own: the pointer moving over a grid
//! redraws the tile it enters and the one it leaves, and nothing else.

use std::cell::Cell;
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
            // The heart's 30-point disc, 7 in from the corner.
            let heart = side - 7.0 - 30.0..side - 7.0;
            let shown = !self.state.selecting;
            return Some(if shown && heart.contains(&x) && heart.contains(&y) {
                Part::Heart
            } else {
                Part::Sleeve
            });
        }
        let look = crate::theme::Look::of(window, cx);
        let title = f32::from(look.line_height(Type::Meta));
        let credit = f32::from(look.line_height(Type::Fine));
        let title_top = side + CAPTION_GAP;
        let credit_top = title_top + title + CAPTION_GAP;
        if (title_top..title_top + title).contains(&y) {
            return Some(Part::Title);
        }
        if !self.state.show_artist || !(credit_top..credit_top + credit).contains(&y) {
            return None;
        }
        let size = look.font_size(Type::Fine);
        if self
            .credit_widths
            .as_ref()
            .is_none_or(|(at, _)| *at != size)
        {
            let font = font(crate::theme::FONT_FAMILY);
            let widths = self
                .record
                .credit
                .iter()
                .map(|(text, _)| {
                    let run = TextRun {
                        len: text.len(),
                        font: font.clone(),
                        color: Hsla::default(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    window
                        .text_system()
                        .shape_line(text.clone(), size, &[run], None)
                        .width
                })
                .collect();
            self.credit_widths = Some((size, widths));
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
        let mut art = div().relative().size(side).flex_none().bg(p.surface);
        match &state.sleeve {
            Sleeve::Ready(image) => {
                art = art.child(img(ImageSource::Render(image.clone())).size_full())
            }
            Sleeve::Colour(colour) => art = art.bg(*colour),
            Sleeve::Waiting | Sleeve::None => {
                art = art.child(crate::components::Placeholder::new(side))
            }
        }
        if let Some(codec) = &record.codec {
            // The codec only: at tile size a rate is unreadable, and the codec
            // is what says whether this is the good copy.
            art = art.child(badge(p, codec.clone()).top(px(6.0)).right(px(6.0)));
        }
        if let (Some(release), false) = (&record.badge, state.selecting) {
            art = art.child(badge(p, release.clone()).top(px(6.0)).left(px(6.0)));
        }
        let over_sleeve = matches!(hovered, Some(Part::Sleeve | Part::Heart));
        if state.starting || (over_sleeve && !state.selecting) {
            art = art.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.dim())
                    .child(if state.starting {
                        div()
                            .typeset(p, Type::TitleSmall, p.strong)
                            .child("…")
                            .into_any_element()
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
        if !state.selecting && (hovered.is_some() || state.favourite) {
            art = art.child(
                div()
                    .absolute()
                    .right(px(7.0))
                    .bottom(px(7.0))
                    .size(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if hovered == Some(Part::Heart) {
                        p.hover
                    } else {
                        p.surface
                    })
                    .child(heart_glyph(p, state.favourite)),
            );
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
            .gap(px(CAPTION_GAP))
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, _, _| origin.set(bounds.origin),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
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

fn badge(p: &Paint, text: impl Into<SharedString>) -> Div {
    div()
        .absolute()
        .px(px(6.0))
        .py(px(2.0))
        .bg(p.surface)
        .typeset(p, Type::Fine, p.ink)
        .child(text.into())
}

/// Three bars in the accent: the record is the one playing.
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

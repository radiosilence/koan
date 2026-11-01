//! One record in a grid, shared by the album browser and an artist's page so
//! they behave alike: the sleeve plays the record, the title opens it, each
//! credited artist links to their page. While picking, the whole tile ticks.
//!
//! Hover is drawn by GPUI's group styles rather than tracked state, so the
//! pointer moving over a grid redraws nothing but the tile under it.

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::Album;

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
const LINE_GAP: f32 = 22.0;
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

pub struct TileState {
    pub favourite: bool,
    pub playing: bool,
    pub arriving: bool,
    pub selecting: bool,
    pub selected: bool,
    pub starting: bool,
    pub show_artist: bool,
    /// What dragging the tile carries, and what its menu acts on: the page's
    /// pick when the tile is in it, built once for the pick; itself
    /// otherwise.
    pub drag: PlayableDrag,
    /// Run after a menu verb: ends the pick when the menu acted on one.
    pub after: actions::After,
}

pub fn tile<V: TileHost>(
    p: &Paint,
    album: &Album,
    side: Pixels,
    sleeve: Sleeve,
    state: TileState,
    cx: &mut Context<V>,
) -> AnyElement {
    let id = album.id;
    let view = cx.weak_entity();
    let click = |part: Part| {
        super::listen(
            view.clone(),
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
    if let Sleeve::Ready(image) = sleeve {
        art = art.child(img(ImageSource::Render(image)).size_full());
    }
    if let Some(codec) = &album.codec {
        // The codec only: at tile size a rate is unreadable, and the codec is
        // what says whether this is the good copy.
        art = art.child(badge(p, codec.to_uppercase()).top(px(6.0)).right(px(6.0)));
    }
    if let (Some(release), false) = (&album.release_badge, state.selecting) {
        art = art.child(badge(p, release.clone()).top(px(6.0)).left(px(6.0)));
    }
    // The dim snaps in and out for now; it takes the theme's 120 ms when the
    // motion tokens land.
    if state.starting || !state.selecting {
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
    if let Some(bar) = download_bar(p, album, state.arriving) {
        art = art.child(bar);
    }
    if !state.selecting {
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
            .typeset(
                p,
                Type::Meta,
                if state.favourite { p.accent } else { p.ink },
            )
            .cursor_pointer()
            .on_click(click(Part::Heart))
            .child(heart_glyph(p, state.favourite));
        art = art.child(if state.favourite {
            heart
        } else {
            heart.opacity(0.0).group_hover("tile", |s| s.opacity(1.0))
        });
    }
    if state.selecting {
        // Over the art rather than a border on it, so ticking a tile neither
        // shrinks its sleeve nor moves it.
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
        .child(album.title.clone());

    let mut credit = div()
        .flex()
        .w_full()
        .overflow_hidden()
        .whitespace_nowrap()
        .typeset(p, Type::Fine, p.muted);
    if state.show_artist {
        for (i, part) in album.artist_credit.iter().enumerate() {
            let span = div()
                .id(("credit", (id as u64) << 8 | i as u64))
                .child(part.text.clone());
            credit = credit.child(match part.artist_id {
                Some(artist) if !state.selecting => span
                    .cursor_pointer()
                    .hover(|s| s.underline().text_color(p.ink))
                    .on_click(click(Part::Artist(artist))),
                _ => span,
            });
        }
    }
    if let Some(year) = album.year {
        credit = credit.child(if state.show_artist {
            format!(" · {year}")
        } else {
            year.to_string()
        });
    }

    let drag = state.drag;
    let target = actions::MenuTarget {
        items: drag.items.clone(),
        album: None,
        after: state.after,
    };
    div()
        .id(("tile", id as u64))
        .group("tile")
        .flex()
        .flex_col()
        .w(side)
        .gap(px(CAPTION_GAP))
        .child(art)
        .child(title)
        .child(credit)
        .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
        .on_mouse_down(MouseButton::Right, actions::mark(view, target))
        .into_any_element()
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
fn download_bar(p: &Paint, album: &Album, arriving: bool) -> Option<Div> {
    let fraction = match album.on_device {
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

//! A record's tile in a grid, as the album browser and an artist's page draw
//! it: the grid's geometry (`TileGrid`), the sleeve and what lies over it
//! (`TileArt`), and the measures a tile's owner hit-tests its parts by, since
//! a scrolling grid's tile is one interactive element rather than one per
//! part.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Placeholder, TileColumns};
use crate::theme::tokens::Scale;
use crate::theme::{FONT_FAMILY, Icon, KoanIcon, KoanStyled, Look, Space, TypeRole};

/// The record grid: `TileColumns` across the width, and each row's height.
/// Every tile is the same size, so nothing is measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileGrid {
    pub columns: usize,
    pub side: Pixels,
    /// Between tiles in a row: at least `TileColumns::MIN_GAP`, with a share
    /// of the spare width.
    pub gap: Pixels,
    /// Above the first row and below the last: the page's margin.
    pub margin: Pixels,
    /// Either side of a row: the page's margin and a share of the spare
    /// width.
    pub inset: Pixels,
    /// A row's height, its spacing below included.
    pub row: Pixels,
}

impl TileGrid {
    /// Below a row's captions, before the next row.
    pub const LINE_GAP: f32 = 22.0;
    /// Between sleeve, title and credit.
    pub const CAPTION_GAP: f32 = 7.0;

    /// The grid for a list `width` wide, with captions on `scale`. The
    /// margin is the page's, so the grid's edge is the header's and the
    /// rows'.
    pub fn for_width(width: Pixels, scale: &Scale) -> Self {
        let margin = px(Space::Xxl.points() * scale.space);
        let columns = TileColumns::for_width(width, margin);
        Self {
            columns: columns.columns,
            side: columns.side,
            gap: columns.gap,
            margin,
            inset: columns.inset,
            row: columns.side + Self::caption_height(scale) + px(Self::LINE_GAP),
        }
    }

    /// Whether `other` places the rows as this does: the same columns and
    /// row height, so a row is where it was and only the room between tiles
    /// moved.
    pub fn same_rows(&self, other: &TileGrid) -> bool {
        self.columns == other.columns && self.row == other.row
    }

    pub fn rows(&self, items: usize) -> usize {
        items.div_ceil(self.columns)
    }

    /// Between sleeve, title and credit, as laid out.
    pub fn caption_gap() -> Pixels {
        px(Self::CAPTION_GAP)
    }

    /// A tile's height: its sleeve and caption, without the gap below.
    pub fn tile_height(&self) -> Pixels {
        self.row - px(Self::LINE_GAP)
    }

    /// The caption under a sleeve: title in `meta`, credit in `fine`, each
    /// after `CAPTION_GAP`.
    pub fn caption_height(scale: &Scale) -> Pixels {
        let line = |role: TypeRole| role.points() * scale.text * role.line_height();
        px(Self::CAPTION_GAP + line(TypeRole::Meta) + Self::CAPTION_GAP + line(TypeRole::Fine))
    }

    /// Where a tile's title and credit lines start and how tall each is,
    /// below the sleeve's top: what the tile's owner tells its parts apart
    /// by.
    pub fn caption_lines(look: &Look, side: Pixels) -> (Range, Range) {
        let gap = Self::CAPTION_GAP;
        let title = f32::from(look.line_height(TypeRole::Meta));
        let credit = f32::from(look.line_height(TypeRole::Fine));
        let title_top = f32::from(side) + gap;
        let credit_top = title_top + title + gap;
        (
            title_top..title_top + title,
            credit_top..credit_top + credit,
        )
    }
}

type Range = std::ops::Range<f32>;

/// The sleeve as a tile shows it.
pub enum TileSleeve {
    /// The picture, `shown` from 0 to 1 while it fades in over `ground`,
    /// the record's colour shown before it.
    Image {
        image: Arc<RenderImage>,
        shown: f32,
        ground: Option<Hsla>,
    },
    /// The picture's average colour, standing in while it uploads.
    Colour(Hsla),
    /// No picture, or none yet: the ensō placeholder.
    Missing,
}

/// What dims the sleeve: playing it, under the pointer, or starting to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TileMark {
    Play,
    Starting,
}

/// The sleeve square and everything over it: the codec top right and the
/// release's badge top left, the play mark over a dim, the playing bars
/// bottom left, the share on this device as a bar along the foot, the heart
/// on its disc bottom right, and while picking the tick and the picked
/// outline. The sleeve keeps its place and size whatever lies over it.
#[derive(IntoElement)]
pub struct TileArt {
    side: Pixels,
    sleeve: TileSleeve,
    codec: Option<SharedString>,
    badge: Option<SharedString>,
    mark: Option<TileMark>,
    playing: Option<AnyElement>,
    on_device: Option<f32>,
    heart: Option<(bool, bool)>,
    pick: Option<bool>,
}

impl TileArt {
    /// The heart's disc, in points, and its inset from the corner.
    pub const HEART: f32 = 30.0;
    pub const HEART_INSET: f32 = 7.0;
    const BADGE_INSET: f32 = 6.0;
    const BARS_INSET: f32 = 8.0;
    const BAR: f32 = 3.0;
    /// The pick tick's glyph, in points as drawn.
    const TICK: f32 = 20.0;
    /// What the play mark dims the sleeve with.
    const DIM: f32 = 0.35;

    pub fn new(side: Pixels, sleeve: TileSleeve) -> Self {
        Self {
            side,
            sleeve,
            codec: None,
            badge: None,
            mark: None,
            playing: None,
            on_device: None,
            heart: None,
            pick: None,
        }
    }

    /// The codec, top right: at tile size a rate is unreadable, and the
    /// codec is what says whether this is the good copy.
    pub fn codec(mut self, codec: Option<SharedString>) -> Self {
        self.codec = codec;
        self
    }

    /// The release's badge, top left ("deluxe").
    pub fn badge(mut self, badge: Option<SharedString>) -> Self {
        self.badge = badge;
        self
    }

    pub fn mark(mut self, mark: Option<TileMark>) -> Self {
        self.mark = mark;
        self
    }

    /// The playing bars, bottom left, while the record plays.
    pub fn playing(mut self, bars: Option<AnyElement>) -> Self {
        self.playing = bars;
        self
    }

    /// How much of the record is on this device, 0 to 1, while that is
    /// worth showing: a bar along the foot.
    pub fn on_device(mut self, fraction: Option<f32>) -> Self {
        self.on_device = fraction;
        self
    }

    /// The heart: whether it is a favourite, and whether the pointer is on
    /// the heart's disc.
    pub fn heart(mut self, heart: Option<(bool, bool)>) -> Self {
        self.heart = heart;
        self
    }

    /// While picking: whether this record is picked.
    pub fn pick(mut self, picked: Option<bool>) -> Self {
        self.pick = picked;
        self
    }

    /// Whether `(x, y)`, from the sleeve's top left, is on the heart's disc.
    pub fn on_heart(side: Pixels, x: f32, y: f32) -> bool {
        let side = f32::from(side);
        let disc = side - Self::HEART_INSET - Self::HEART..side - Self::HEART_INSET;
        disc.contains(&x) && disc.contains(&y)
    }
}

impl RenderOnce for TileArt {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let pal = &look.palette;
        let side = self.side;
        let mut art = div()
            .relative()
            .size(side)
            .flex_none()
            .bg(pal.surface.hsla());
        match self.sleeve {
            TileSleeve::Image {
                image,
                shown,
                ground,
            } => {
                if shown < 1.0
                    && let Some(ground) = ground
                {
                    art = art.bg(ground);
                }
                art = art.child(img(ImageSource::Render(image)).size_full().opacity(shown));
            }
            TileSleeve::Colour(colour) => art = art.bg(colour),
            TileSleeve::Missing => art = art.child(Placeholder::new(side)),
        }
        let badge = |text: SharedString| {
            div()
                .absolute()
                .top(px(Self::BADGE_INSET))
                .px(px(Self::BADGE_INSET))
                .py(px(2.0))
                .bg(pal.surface.hsla())
                .text_role(TypeRole::Fine, &look)
                .text_color(pal.ink.hsla())
                .child(text)
        };
        if let Some(codec) = self.codec {
            art = art.child(badge(codec).right(px(Self::BADGE_INSET)));
        }
        if let (Some(release), None) = (self.badge, self.pick) {
            art = art.child(badge(release).left(px(Self::BADGE_INSET)));
        }
        if let Some(mark) = self.mark {
            art = art.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui_kit::black().opacity(Self::DIM))
                    .child(match mark {
                        TileMark::Starting => div()
                            .text_role(TypeRole::TitleSmall, &look)
                            .text_color(pal.strong.hsla())
                            .child("…")
                            .into_any_element(),
                        TileMark::Play => KoanIcon::new(Icon::PlayMark)
                            .role(TypeRole::TitleSmall)
                            .color(pal.strong.hsla())
                            .into_any_element(),
                    }),
            );
        }
        if let Some(bars) = self.playing {
            art = art.child(
                div()
                    .absolute()
                    .left(px(Self::BARS_INSET))
                    .bottom(px(Self::BARS_INSET))
                    .child(bars),
            );
        }
        if let Some(fraction) = self.on_device {
            art = art.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(Self::BAR))
                    .bg(pal.rule.hsla())
                    .child(
                        div()
                            .h_full()
                            .w(relative(fraction.clamp(0.0, 1.0)))
                            .bg(look.accent.hsla()),
                    ),
            );
        }
        if let (Some((favourite, hovered)), None) = (self.heart, self.pick) {
            let glyph = if favourite {
                KoanIcon::new(Icon::Favourited)
                    .role(TypeRole::Meta)
                    .color(look.accent.hsla())
            } else {
                KoanIcon::new(Icon::Favourite)
                    .role(TypeRole::Meta)
                    .color(pal.ink.hsla())
            };
            art = art.child(
                div()
                    .absolute()
                    .right(px(Self::HEART_INSET))
                    .bottom(px(Self::HEART_INSET))
                    .size(px(Self::HEART))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if hovered {
                        pal.hover.hsla()
                    } else {
                        pal.surface.hsla()
                    })
                    .child(glyph),
            );
        }
        if let Some(picked) = self.pick {
            // Over the art rather than a border on it, so ticking a tile
            // neither shrinks its sleeve nor moves it.
            if picked {
                art = art.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_3()
                        .border_color(look.accent.hsla()),
                );
            }
            // White over a fill, the accent when picked.
            let tick = if picked {
                KoanIcon::new(Icon::Picked).fill(look.accent.hsla())
            } else {
                KoanIcon::new(Icon::Unpicked).fill(gpui_kit::black().opacity(0.25))
            };
            art = art.child(
                div()
                    .absolute()
                    .top(px(Self::HEART_INSET))
                    .left(px(Self::HEART_INSET))
                    .child(tick.points(Self::TICK / 1.2).color(gpui_kit::white())),
            );
        }
        art
    }
}

/// The widths `texts` take side by side in `role`, as drawn: for telling
/// apart which of a line's names the pointer is on.
pub fn text_widths(
    texts: &[SharedString],
    role: TypeRole,
    window: &Window,
    cx: &App,
) -> Vec<Pixels> {
    let look = Look::of(window, cx);
    let size = look.font_size(role);
    let font = font(FONT_FAMILY);
    texts
        .iter()
        .map(|text| {
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
        .collect()
}

/// An element filling its parent that records where the parent was laid
/// out, for an owner hit-testing parts of it by position.
pub fn origin_probe(origin: Rc<Cell<Point<Pixels>>>) -> impl IntoElement {
    canvas(
        move |bounds, _, _| origin.set(bounds.origin),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_keep_their_size_and_the_room_around_them_takes_the_width() {
        let scale = Scale::of(100);
        let narrow = TileGrid::for_width(px(300.0), &scale);
        assert_eq!(narrow.columns, 1);
        let a = TileGrid::for_width(px(1200.0), &scale);
        let b = TileGrid::for_width(px(1240.0), &scale);
        assert_eq!(
            a.side, b.side,
            "a resize moves tiles apart, never grows them"
        );
        assert_eq!(a.columns, b.columns);
        assert!(b.gap > a.gap && b.inset > a.inset);
        assert!(a.gap >= px(TileColumns::MIN_GAP));
        for (grid, width) in [(a, 1200.0), (b, 1240.0)] {
            let used = grid.side * grid.columns as f32
                + grid.gap * (grid.columns as f32 - 1.0)
                + grid.inset * 2.0;
            assert!((f32::from(used) - width).abs() < 1.0);
        }
        // Wide enough for one more tile at the least gap: a column more.
        let more = TileGrid::for_width(a.side * 2.0 + a.gap * 3.0 + px(2000.0), &scale);
        assert!(more.columns > a.columns);
        assert_eq!(a.rows(0), 0);
        assert_eq!(a.rows(a.columns + 1), 2);
    }
}

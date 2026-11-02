//! Where rows and pages stand: the Mac's `RowMetrics`, and the page margin
//! the spec gives. Row heights follow the text size alone, their padding
//! and the margins the spacing scale, as on the Mac. Read from the window's
//! `Look`, so nothing passes them by hand.

use gpui_kit::{Pixels, px};

use crate::theme::{Look, Scale, Space, TypeRole};

/// A row's height, by what it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// One line: the title alone.
    Line,
    /// The title over a line of credit.
    Text,
    /// A sleeve beside the title and its credit.
    Art,
    /// A record's heading over its run in a queue or playlist: its sleeve
    /// (`RowMetrics::heading_sleeve`), the record over its artist and a
    /// line of detail.
    Heading,
}

pub struct RowMetrics;

/// The playing bars, the Mac's: three 2-point bars 2 points apart, 11 tall.
pub struct BarMetrics;

impl BarMetrics {
    pub const COUNT: usize = 3;
    pub const BAR: f32 = 2.0;
    pub const GAP: f32 = 2.0;
    pub const WIDTH: Pixels =
        px(Self::COUNT as f32 * Self::BAR + (Self::COUNT - 1) as f32 * Self::GAP);
    pub const HEIGHT: Pixels = px(11.0);
}

/// The transport bar's figures, the spec's: 64 tall, a 40-point cover, the
/// controls 22 apart over the seek bar.
pub struct TransportMetrics;

impl TransportMetrics {
    /// The Mac's: 9 points above and below a 46-point zone, the rule drawn
    /// inside its top.
    pub const HEIGHT: Pixels = px(64.0);
    pub const COVER: Pixels = px(40.0);
    /// The middle column, the controls over the seek bar.
    pub const CENTRE: Pixels = px(520.0);
    /// The seek bar's height: a compact slider's.
    pub const SEEK: Pixels = px(20.0);

    /// Between the controls' glyphs, as the Mac spaces them.
    pub fn control_gap(look: &Look) -> Pixels {
        look.spaced(22.0)
    }

    /// Between the trailing group's items (the format badge, sleep, play on,
    /// output, EQ), the Mac's. Each item is as wide as its glyph or label in
    /// the layout, its hit area reaching half this gap either side.
    pub fn trailing_gap(look: &Look) -> Pixels {
        look.spaced(12.0)
    }

    /// The play button's outline: an outlined icon button around a `body`
    /// glyph, its padding (the hairline drawn inside it) included.
    pub fn play_outline(look: &Look) -> Pixels {
        px(crate::theme::icons::side(
            look.points(crate::theme::TypeRole::Body),
        )) + look.spaced(7.0) * 2.0
    }

    /// The seek row as the Mac lays it out: its line's reach, 14 points.
    const SEEK_ROW: f32 = 14.0;
    /// Between the controls and the seek row, the Mac's stack spacing.
    const STACK_GAP: f32 = 5.0;

    /// Where the centre column's two rows stand, from the bar's top (its
    /// rule included): the play button's outline and the seek line's centre.
    /// The controls, the gap and the seek row are centred in the bar, so the
    /// space above the outline equals the space below the seek row, as the
    /// Mac's transport measures at 2x (7.5 points each in its 64).
    pub fn centre_rows(look: &Look) -> CentreRows {
        let outline = Self::play_outline(look);
        let gap = look.spaced(Self::STACK_GAP);
        let row = look.spaced(Self::SEEK_ROW);
        let margin = ((Self::HEIGHT - outline - gap - row) / 2.).max(px(0.));
        CentreRows {
            outline_top: margin,
            outline,
            track_centre: margin + outline + gap + row / 2.,
        }
    }

    /// A trailing glyph (sleep, play on, output, EQ), the Mac's 17 points
    /// at the chosen text size: larger than the `body` glyphs beside the
    /// bar's text, as the Mac draws its device and output buttons.
    pub fn trailing_icon(look: &Look) -> f32 {
        17.0 * look.scale.size
    }
}

/// The centre column's rows, from `TransportMetrics::centre_rows`.
#[derive(Clone, Copy, Debug)]
pub struct CentreRows {
    pub outline_top: Pixels,
    pub outline: Pixels,
    pub track_centre: Pixels,
}

/// A grid of record tiles across a width: how many columns, how big each
/// tile, and the room between and beside them.
///
/// The tiles keep one size whatever the width. A window being resized moves
/// the tiles apart rather than growing them: the spare width is shared out
/// equally among the gaps and the two side margins, so the grid's edge stays
/// near the page's margin, and a column is added only once another tile fits
/// at the least gap, or dropped once one no longer does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileColumns {
    pub columns: usize,
    pub side: Pixels,
    pub gap: Pixels,
    /// Each side's margin: the page's, and its share of the spare width.
    pub inset: Pixels,
}

impl TileColumns {
    /// A tile's side, in points.
    pub const SIDE: f32 = 160.0;
    /// The least gap between tiles in a row.
    pub const MIN_GAP: f32 = 18.0;

    /// The columns for a list `width` wide with `margin` at either side.
    /// Narrower than one tile, the one tile shrinks to fit.
    pub fn for_width(width: Pixels, margin: Pixels) -> Self {
        let margin = f32::from(margin);
        let usable = (f32::from(width) - 2.0 * margin).max(1.0);
        let columns =
            (((usable + Self::MIN_GAP) / (Self::SIDE + Self::MIN_GAP)).floor() as usize).max(1);
        let side = Self::SIDE.min(usable.floor());
        let spare =
            (usable - side * columns as f32 - Self::MIN_GAP * (columns as f32 - 1.0)).max(0.0);
        // The gaps between columns and the two margins share the spare width.
        let share = spare / (columns as f32 + 1.0);
        Self {
            columns,
            side: px(side),
            gap: px(if columns > 1 {
                Self::MIN_GAP + share
            } else {
                0.0
            }),
            inset: px(margin + if columns > 1 { share } else { spare / 2.0 }),
        }
    }
}

/// A record heading's sleeve at 100 %, the Mac queue's.
const HEADING_SLEEVE: f32 = 52.0;

impl RowMetrics {
    pub fn height(kind: RowKind, look: &Look) -> Pixels {
        let points = match kind {
            RowKind::Line => 24.0,
            RowKind::Text => 34.0,
            RowKind::Art => 40.0,
            RowKind::Heading => HEADING_SLEEVE + 12.0,
        };
        px((points * look.scale.size).round()) + Self::padding(look) * 2.0
    }

    /// A sidebar row's height, the Mac's sidebar measured at 2x: a place
    /// (one line) or a playlist (its sleeve beside its name over a detail
    /// line), the content centred in it.
    pub fn nav_height(detail: bool, look: &Look) -> Pixels {
        let points = if detail { 39.0 } else { 34.0 };
        px((points * look.scale.size).round())
    }

    /// A navigation row's face, square: a playlist's sleeve in the sidebar.
    pub fn nav_art(look: &Look) -> Pixels {
        px((24.0 * look.scale.size).round())
    }

    /// A row's height at 100 %, for what has no window to ask: a list's
    /// guess for the rows it has not measured yet.
    pub fn estimate(kind: RowKind) -> Pixels {
        let points = match kind {
            RowKind::Line => 24.0,
            RowKind::Text => 34.0,
            RowKind::Art => 40.0,
            RowKind::Heading => HEADING_SLEEVE + 12.0,
        };
        px(points + 8.0)
    }

    /// A record page's sleeve in its header, square, the Mac's: fixed
    /// whatever the window's width, so a resize never moves it.
    pub fn header_art(look: &Look) -> Pixels {
        px((132.0 * look.scale.size).round())
    }

    /// An artist's photo in their page's header, the Mac's.
    pub fn artist_photo(look: &Look) -> Pixels {
        px((112.0 * look.scale.size).round())
    }

    /// A record heading's sleeve, square.
    pub fn heading_sleeve(look: &Look) -> Pixels {
        px((HEADING_SLEEVE * look.scale.size).round())
    }

    /// A row's leading glyph, where it has no number: the glyph's own
    /// width, so it starts on the page's edge.
    pub fn lead_glyph(look: &Look) -> Pixels {
        px(crate::theme::icons::side(
            look.points(crate::theme::TypeRole::Body),
        ))
    }

    /// A row's sleeve, square.
    pub fn sleeve(look: &Look) -> Pixels {
        px((32.0 * look.scale.size).round())
    }

    /// Above and below a row's content.
    pub fn padding(look: &Look) -> Pixels {
        look.space(Space::Xs)
    }

    /// A page's margin on every side, and what a row's content is inset by
    /// so it lines up under the page's header. One figure, so the page's
    /// edge is tuned in one place; 16 for now, where the spec gives 32.
    pub fn page_margin(look: &Look) -> Pixels {
        look.space(Space::L)
    }

    /// Above a page's header: the page's margin, as the spec has it on
    /// every side.
    pub fn page_top(look: &Look) -> Pixels {
        Self::page_margin(look)
    }

    /// Between a page's header and its list.
    pub fn header_gap(look: &Look) -> Pixels {
        look.space(Space::M)
    }

    /// The sidebar's width as it opens, and what a double click on its edge
    /// restores; a drag keeps it between `sidebar_min` and `sidebar_max`.
    pub const SIDEBAR: Pixels = px(220.);
    pub const SIDEBAR_MIN: Pixels = px(180.);
    pub const SIDEBAR_MAX: Pixels = px(360.);

    /// The window's least size: the sidebar beside a page wide enough for a
    /// track row's columns.
    pub const WINDOW_MIN: gpui_kit::Size<Pixels> = gpui_kit::Size {
        width: px(220. + 640.),
        height: px(420.),
    };

    /// An inspector's width beside the page, the Mac's ideal for its
    /// lyrics.
    pub fn inspector(look: &Look) -> Pixels {
        look.spaced(280.)
    }

    /// Between columns of a row.
    pub fn column_gap(look: &Look) -> Pixels {
        look.space(Space::M)
    }
}

/// Record tiles in rows: `TileColumns` across the width, and each row's
/// height with the caption under the sleeve and the gap below it. Every tile
/// is the same size, so nothing is measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileGrid {
    pub columns: usize,
    pub side: Pixels,
    pub gap: Pixels,
    /// Either side of a row: the margin asked for and a share of the spare
    /// width.
    pub inset: Pixels,
    /// A row's height, its spacing below included.
    pub row: Pixels,
}

impl TileGrid {
    /// Below a row of tiles, before the next.
    pub const LINE_GAP: f32 = 22.0;
    /// Between sleeve, title and credit.
    pub const CAPTION_GAP: f32 = 7.0;

    /// The grid for a list `width` wide that keeps `margin` clear at each
    /// side, with captions on `scale`.
    pub fn inside(width: Pixels, margin: Pixels, scale: &Scale) -> Self {
        let columns = TileColumns::for_width(width, margin);
        Self {
            columns: columns.columns,
            side: columns.side,
            gap: columns.gap,
            inset: columns.inset,
            row: columns.side + Self::caption_height(scale) + px(Self::LINE_GAP),
        }
    }

    /// A tile's own height: its sleeve and caption, without the gap below.
    pub fn tile_height(&self) -> Pixels {
        self.row - px(Self::LINE_GAP)
    }

    /// The title and credit under a sleeve.
    pub fn caption_height(scale: &Scale) -> Pixels {
        let line = |role: TypeRole| role.points() * scale.text * role.line_height();
        px(Self::CAPTION_GAP + line(TypeRole::Meta) + Self::CAPTION_GAP + line(TypeRole::Fine))
    }
}

#[cfg(test)]
mod tile_grid_tests {
    use super::*;

    #[test]
    fn rows_hold_the_columns_sleeve_and_caption() {
        let scale = Scale::of(100);
        let grid = TileGrid::inside(px(900.), px(16.), &scale);
        let columns = TileColumns::for_width(px(900.), px(16.));
        assert_eq!(grid.columns, columns.columns);
        assert_eq!(grid.inset, columns.inset);
        assert_eq!(
            grid.tile_height(),
            grid.side + TileGrid::caption_height(&scale)
        );
    }
}

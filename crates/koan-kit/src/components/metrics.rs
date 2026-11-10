//! Where rows and pages stand: the Mac's `RowMetrics`, and the page margin
//! the spec gives. Row heights follow the text size alone, their padding
//! and the margins the spacing scale, as on the Mac. Read from the window's
//! `Look`, so nothing passes them by hand.

use gpui::{Pixels, Window, font, px};

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

    /// The seek row as the Mac lays it out: its line's reach, 14 points.
    const SEEK_ROW: f32 = 14.0;
    /// Between the controls and the seek row, the Mac's stack spacing.
    const STACK_GAP: f32 = 5.0;

    /// Where the centre column's rows stand, measured from below the bar's
    /// rule, which the column sits under: the play button's outline, the
    /// controls' hit row and the seek row. The outline, the gap and the seek
    /// line's row are centred in the bar, so the space above the outline
    /// equals the space below the seek row, as the Mac's transport measures
    /// at 2x (7.5 points each in its 64). The controls' hit row is centred on
    /// the outline, as tall as `HIT` where there is room and never reaching
    /// above the rule or into the seek row, so a click in it never seeks.
    pub fn centre_rows(look: &Look) -> CentreRows {
        let inner = Self::HEIGHT - look.hairline;
        let outline = super::ButtonKind::IconOutlined.glyph_side(look);
        let gap = look.spaced(Self::STACK_GAP);
        let row = look.spaced(Self::SEEK_ROW);
        let margin = ((inner - outline - gap - row) / 2.).max(px(0.));
        let track_centre = margin + outline + gap + row / 2.;
        let seek_top = (track_centre - Self::SEEK / 2.).max(margin + outline);
        let centre = margin + outline / 2.;
        let half = (crate::theme::HIT / 2.).min(centre).min(seek_top - centre);
        CentreRows {
            outline_top: margin,
            outline,
            controls_top: centre - half,
            controls_height: half * 2.,
            seek_top,
            track_centre,
        }
    }

    /// A trailing glyph (sleep, play on, output, EQ), the Mac's 17 points
    /// at the chosen text size: larger than the `body` glyphs beside the
    /// bar's text, as the Mac draws its device and output buttons.
    pub fn trailing_icon(look: &Look) -> f32 {
        17.0 * look.scale.size
    }
}

/// The centre column's rows, from `TransportMetrics::centre_rows`, below
/// the bar's rule.
#[derive(Clone, Copy, Debug)]
pub struct CentreRows {
    pub outline_top: Pixels,
    pub outline: Pixels,
    /// The controls' hit row: they fill it, their glyphs on its middle.
    pub controls_top: Pixels,
    pub controls_height: Pixels,
    /// The seek row's top, `SEEK` tall.
    pub seek_top: Pixels,
    pub track_centre: Pixels,
}

/// A grid of record tiles across a width: how many columns, how big each
/// tile, and the room between and beside them.
///
/// The tiles keep one size whatever the width. A window being resized moves
/// the tiles apart rather than growing them: the spare width is shared out
/// equally among the gaps, so the grid's first and last columns stand on the
/// page's margin, in line with its header and rows, and a column is added
/// only once another tile fits at the least gap, or dropped once one no
/// longer does. One tile alone stands on the left margin too, the spare
/// width beside it, so the first column is level with the header at every
/// width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileColumns {
    pub columns: usize,
    pub side: Pixels,
    pub gap: Pixels,
    /// Each side's margin: the page's.
    pub inset: Pixels,
}

impl TileColumns {
    /// A tile's side, in points.
    pub const SIDE: f32 = 160.0;
    /// The least gap between tiles in a row.
    pub const MIN_GAP: f32 = 18.0;
    /// The smallest a tile gets when the tiles fill the row.
    pub const LEAST_FILLING: f32 = 150.0;

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
        // The gaps between columns share the spare width; the margins stay
        // the page's.
        Self {
            columns,
            side: px(side),
            gap: px(if columns > 1 {
                Self::MIN_GAP + spare / (columns as f32 - 1.0)
            } else {
                0.0
            }),
            inset: px(margin),
        }
    }

    /// The columns of a tablet's page, whose width is one of a few and does
    /// not follow a pointer: the tiles grow to take the row, so two columns
    /// never stand a tile apart.
    pub fn filling(width: Pixels, margin: Pixels) -> Self {
        let margin = f32::from(margin);
        let usable = (f32::from(width) - 2.0 * margin).max(1.0);
        let columns = (((usable + Self::MIN_GAP) / (Self::LEAST_FILLING + Self::MIN_GAP)).floor()
            as usize)
            .max(1);
        let gaps = Self::MIN_GAP * (columns as f32 - 1.0);
        let side = ((usable - gaps) / columns as f32).floor();
        Self {
            columns,
            side: px(side),
            gap: px(if columns > 1 {
                (usable - side * columns as f32) / (columns as f32 - 1.0)
            } else {
                0.0
            }),
            inset: px(margin),
        }
    }
}

/// A record heading's sleeve at 100 %, the Mac queue's, and a phone's,
/// larger beside the record's three lines on a narrow screen.
const HEADING_SLEEVE: f32 = 52.0;
const HEADING_SLEEVE_TOUCH: f32 = 64.0;

fn heading_sleeve(look: &Look) -> f32 {
    if look.touch {
        HEADING_SLEEVE_TOUCH
    } else {
        HEADING_SLEEVE
    }
}

impl RowMetrics {
    pub fn height(kind: RowKind, look: &Look) -> Pixels {
        let points = match kind {
            RowKind::Line => 24.0,
            RowKind::Text => 34.0,
            RowKind::Art => 40.0,
            RowKind::Heading => heading_sleeve(look) + 12.0,
        };
        (px((points * look.scale.size).round()) + Self::padding(look) * 2.0).max(look.target())
    }

    /// A sidebar row's height, the Mac's sidebar measured at 2x: a place
    /// (one line) or a playlist (its sleeve beside its name over a detail
    /// line), the content centred in it.
    pub fn nav_height(detail: bool, look: &Look) -> Pixels {
        let points = if detail { 39.0 } else { 34.0 };
        px((points * look.scale.size).round()).max(look.target())
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
        // On a phone the header stacks, and the sleeve heads the page.
        let side = if look.compact { 220.0 } else { 132.0 };
        px((side * look.scale.size).round())
    }

    /// An artist's photo in their page's header: the size of a record's sleeve
    /// in its header, so the two pages line up.
    pub fn artist_photo(look: &Look) -> Pixels {
        Self::header_art(look)
    }

    /// A record heading's sleeve, square.
    pub fn heading_sleeve(look: &Look) -> Pixels {
        px((heading_sleeve(look) * look.scale.size).round())
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

    /// A lead column wide enough for the longest number in a list of
    /// `count` rows, measured in the face the numbers are drawn in (`meta`):
    /// two digits at least.
    pub fn lead_width(count: usize, look: &Look, window: &Window) -> Pixels {
        let digits = count.max(1).to_string().len().max(2) as f32;
        let text = window.text_system();
        let face = text.resolve_font(&font(crate::theme::FONT_FAMILY));
        let size = look.font_size(TypeRole::Meta);
        let digit = text
            .advance(face, size, '0')
            .map(|size| size.width)
            .unwrap_or(size * 0.6);
        digit * digits + px(4.0)
    }

    /// A page's margin on every side, and what a row's content is inset by
    /// so it lines up under the page's header. One figure, so the page's
    /// edge is tuned in one place: 16, as the Swift lists use (the spec gives 32).
    pub fn page_margin(look: &Look) -> Pixels {
        Self::page_margin_on(&look.scale)
    }

    /// `page_margin` for a layout that has the scale and not the look: a
    /// tile grid working out its columns.
    pub fn page_margin_on(scale: &Scale) -> Pixels {
        px(Space::L.points() * scale.space)
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
    /// restores; a drag keeps it between `SIDEBAR_MIN` and `SIDEBAR_MAX`.
    pub const SIDEBAR: Pixels = px(220.);
    pub const SIDEBAR_MIN: Pixels = px(180.);
    pub const SIDEBAR_MAX: Pixels = px(360.);

    /// The window's least size: the sidebar beside a page wide enough for a
    /// track row's columns.
    pub const WINDOW_MIN: gpui::Size<Pixels> = gpui::Size {
        width: px(220. + 640.),
        height: px(420.),
    };

    /// An inspector's width beside the page, the Mac's ideal for its
    /// lyrics, and what a double click on its edge restores; a drag keeps
    /// it between `inspector_min` and `inspector_max`.
    pub fn inspector(look: &Look) -> Pixels {
        look.spaced(280.)
    }

    pub fn inspector_min(look: &Look) -> Pixels {
        look.spaced(220.)
    }

    pub fn inspector_max(look: &Look) -> Pixels {
        look.spaced(560.)
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
    /// The margin asked for: above the first row and below the last.
    pub margin: Pixels,
    /// Either side of a row: the margin asked for.
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
        Self::placed(TileColumns::for_width(width, margin), margin, scale)
    }

    /// The grid for a list `width` wide in this look: a tablet's tiles fill
    /// the row, and any other's keep their size.
    pub fn inside_on(width: Pixels, margin: Pixels, look: &Look) -> Self {
        if look.touch && !look.compact {
            Self::placed(TileColumns::filling(width, margin), margin, &look.scale)
        } else {
            Self::inside(width, margin, &look.scale)
        }
    }

    fn placed(columns: TileColumns, margin: Pixels, scale: &Scale) -> Self {
        Self {
            columns: columns.columns,
            side: columns.side,
            gap: columns.gap,
            margin,
            inset: columns.inset,
            row: columns.side + Self::caption_height(scale) + px(Self::LINE_GAP),
        }
    }

    /// The grid of a page `width` wide: the page's margin clear at each
    /// side, so the grid's edge is the header's and the rows'.
    pub fn for_width(width: Pixels, scale: &Scale) -> Self {
        Self::inside(width, RowMetrics::page_margin_on(scale), scale)
    }

    /// The grid of a page `width` wide in this look, as `for_width`.
    pub fn for_look(width: Pixels, look: &Look) -> Self {
        Self::inside_on(width, RowMetrics::page_margin_on(&look.scale), look)
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

    /// Where a tile's title and credit lines run, down from the sleeve's
    /// top: what the tile's owner tells its parts apart by.
    pub fn caption_lines(
        look: &Look,
        side: Pixels,
    ) -> (std::ops::Range<f32>, std::ops::Range<f32>) {
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
mod transport_tests {
    use super::*;
    use crate::theme::{Accent, Palette};

    fn look(text_size: u32) -> Look {
        Look {
            dark: true,
            contrast: false,
            palette: Palette::of(true),
            accent: Accent::of(None).shade(true),
            scale: Scale::of(text_size),
            icons: true,
            hairline: crate::theme::HAIRLINE,
            opaque: false,
            touch: false,
        }
    }

    #[test]
    fn the_rows_stay_in_the_bar_and_apart_at_every_text_size() {
        for size in (*crate::theme::tokens::TEXT_SIZES.start()
            ..=*crate::theme::tokens::TEXT_SIZES.end())
            .step_by(5)
        {
            let look = look(size);
            let rows = TransportMetrics::centre_rows(&look);
            let inner = TransportMetrics::HEIGHT - crate::theme::HAIRLINE;
            assert!(
                rows.controls_top >= px(0.),
                "{size}%: controls above the rule"
            );
            assert!(
                rows.controls_top + rows.controls_height <= rows.seek_top + px(0.01),
                "{size}%: the controls reach into the seek row"
            );
            assert!(
                rows.seek_top + TransportMetrics::SEEK <= inner + px(0.01) || size > 120,
                "{size}%: the seek row below the bar"
            );
            let centre = rows.outline_top + rows.outline / 2.;
            assert!(
                (rows.controls_top + rows.controls_height / 2. - centre).abs() < px(0.01),
                "{size}%: the controls' glyphs off the outline's middle"
            );
        }
    }

    #[test]
    fn at_100_percent_the_space_above_the_outline_equals_that_below_the_seek_row() {
        let look = look(100);
        let rows = TransportMetrics::centre_rows(&look);
        let inner = TransportMetrics::HEIGHT - crate::theme::HAIRLINE;
        let row = look.spaced(TransportMetrics::SEEK_ROW);
        let below = inner - (rows.track_centre + row / 2.);
        assert!((rows.outline_top - below).abs() < px(0.01));
    }
}

#[cfg(test)]
mod tile_grid_tests {
    use super::*;

    #[test]
    fn filling_tiles_take_the_row() {
        for width in [520.0, 600.0, 802.0, 1144.0] {
            let columns = TileColumns::filling(px(width), px(16.));
            let row = columns.side * columns.columns as f32
                + columns.gap * (columns.columns as f32 - 1.0);
            assert_eq!(row + px(32.), px(width));
            assert!(columns.side >= px(TileColumns::LEAST_FILLING));
            assert!(columns.gap < px(TileColumns::MIN_GAP + 1.0));
        }
    }

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

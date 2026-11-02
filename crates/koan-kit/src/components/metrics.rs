//! Where rows and pages stand: the Mac's `RowMetrics`, and the page margin
//! the spec gives. Row heights follow the text size alone, their padding
//! and the margins the spacing scale, as on the Mac. Read from the window's
//! `Look`, so nothing passes them by hand.

use gpui_kit::{Pixels, px};

use crate::theme::{Look, Space};

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

/// The transport bar's figures, the spec's: 68 tall, a 40-point cover, the
/// controls 22 apart over the seek bar.
pub struct TransportMetrics;

impl TransportMetrics {
    pub const HEIGHT: Pixels = px(68.0);
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

    /// From the bar's top rule to the play button's outline, the Mac's
    /// measured at 2x.
    pub fn controls_top(look: &Look) -> Pixels {
        look.spaced(6.5)
    }

    /// The play button's outline: an outlined icon button around a `body`
    /// glyph, its padding (the hairline drawn inside it) included.
    pub fn play_outline(look: &Look) -> Pixels {
        px(crate::theme::icons::side(
            look.points(crate::theme::TypeRole::Body),
        )) + look.spaced(7.0) * 2.0
    }

    /// From the play button's outline to the seek line's centre, the Mac's:
    /// its stack puts 5 points between the controls and a 14-point seek row.
    pub fn outline_to_track(look: &Look) -> Pixels {
        look.spaced(12.0)
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

    /// Between columns of a row.
    pub fn column_gap(look: &Look) -> Pixels {
        look.space(Space::M)
    }
}

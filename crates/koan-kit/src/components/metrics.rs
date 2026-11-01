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
}

pub struct RowMetrics;

impl RowMetrics {
    pub fn height(kind: RowKind, look: &Look) -> Pixels {
        let points = match kind {
            RowKind::Line => 24.0,
            RowKind::Text => 34.0,
            RowKind::Art => 40.0,
        };
        px((points * look.scale.size).round()) + Self::padding(look) * 2.0
    }

    /// A navigation row's face, square: a playlist's sleeve in the sidebar.
    pub fn nav_art(look: &Look) -> Pixels {
        px((24.0 * look.scale.size).round())
    }

    /// A row's sleeve, square.
    pub fn sleeve(look: &Look) -> Pixels {
        px((32.0 * look.scale.size).round())
    }

    /// Above and below a row's content.
    pub fn padding(look: &Look) -> Pixels {
        look.space(Space::Xs)
    }

    /// A page's margin at its sides, and what a row's content is inset by so
    /// it lines up under the page's header. The spec's 32 on the Mac; the
    /// Swift pages use 16 for lists and 20 for grids, and the spec wins.
    pub fn page_margin(look: &Look) -> Pixels {
        look.space(Space::Xxl)
    }

    /// Above a page's header.
    pub fn page_top(look: &Look) -> Pixels {
        look.space(Space::L)
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

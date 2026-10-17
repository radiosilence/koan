//! The rows of the library's lists, laid out on columns that every row and
//! section of one list shares, so an artist or an album sits at the same x
//! wherever it appears on the page. Text is measured in cells, not chars, and
//! cut with an ellipsis.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::text::fit;
use super::theme::Theme;

/// Draws `text` at `x`, `y` in at most `width` cells, cut with an ellipsis;
/// returns where it landed, for a link to be hit.
pub fn put(buf: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) -> Rect {
    if width == 0 || text.is_empty() {
        return Rect::new(x, y, 0, 0);
    }
    let shown = fit(text, width as usize);
    let (end, _) = buf.set_stringn(x, y, &shown, width as usize, style);
    Rect::new(x, y, end.saturating_sub(x), 1)
}

/// Draws `text` ending at `right` (exclusive), as durations and counts are.
pub fn put_right(buf: &mut Buffer, right: u16, y: u16, width: u16, text: &str, style: Style) {
    let shown = fit(text, width as usize);
    let w = shown.width() as u16;
    buf.set_stringn(right.saturating_sub(w), y, &shown, w as usize, style);
}

/// A link's style: underlined while the pointer is over it.
pub fn link(style: Style, hovered: bool) -> Style {
    if hovered {
        style.add_modifier(Modifier::UNDERLINED)
    } else {
        style
    }
}

/// A run of cells across a row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Span {
    pub x: u16,
    pub w: u16,
}

/// A list's columns. Every row takes the bar and heart at the left; then a
/// lead (a track's number) right-aligned, the name, the artist and a detail
/// (a track's album), and a tail right-aligned at the end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Columns {
    pub heart: u16,
    pub lead: Span,
    pub name: Span,
    pub artist: Span,
    pub detail: Span,
    pub tail: Span,
}

/// The gap between two columns.
const GAP: u16 = 2;

impl Columns {
    /// Columns across `area`: `lead` cells for numbers, `tail` for the
    /// right-hand figures, and an artist and a detail column when the list
    /// has them. The name takes what is left of a fair share.
    pub fn new(area: Rect, lead: u16, tail: u16, artist: bool, detail: bool) -> Self {
        let x = area.x;
        let end = area.x + area.width;
        let heart = x + 1;
        let lead_x = x + 3;
        let name_x = lead_x + if lead > 0 { lead + 1 } else { 0 };
        // One cell is kept clear at the right edge.
        let tail_end = end.saturating_sub(1);
        let tail_x = tail_end.saturating_sub(tail);
        let room = tail_x.saturating_sub(name_x + if tail > 0 { GAP } else { 0 });
        // Narrow lists drop the detail, then the artist, rather than cut each
        // column to nothing.
        let (artist, detail) = match room {
            0..=23 => (false, false),
            24..=49 => (artist, false),
            _ => (artist, detail),
        };
        // The name weighs four, the artist and the detail three each.
        let weight = 4 + 3 * u16::from(artist) + 3 * u16::from(detail);
        let gaps = GAP * (u16::from(artist) + u16::from(detail));
        let share = room.saturating_sub(gaps);
        let artist_w = if artist {
            (u32::from(share) * 3 / u32::from(weight)) as u16
        } else {
            0
        };
        let detail_w = if detail { artist_w } else { 0 };
        let name_w = share.saturating_sub(artist_w + detail_w);
        let artist_x = name_x + name_w + GAP;
        let detail_x = artist_x + artist_w + GAP;
        Columns {
            heart,
            lead: Span { x: lead_x, w: lead },
            name: Span {
                x: name_x,
                w: name_w,
            },
            artist: Span {
                x: artist_x,
                w: artist_w,
            },
            detail: Span {
                x: detail_x,
                w: detail_w,
            },
            tail: Span { x: tail_x, w: tail },
        }
    }
}

/// How a row is drawn: under the cursor, under the pointer, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    Plain,
    Hovered,
    Selected,
}

impl Emphasis {
    pub fn of(selected: bool, hovered: bool) -> Self {
        if selected {
            Emphasis::Selected
        } else if hovered {
            Emphasis::Hovered
        } else {
            Emphasis::Plain
        }
    }

    /// Fills the row's ground, with the cursor's bar at its left.
    pub fn ground(self, buf: &mut Buffer, area: Rect, theme: &Theme) {
        let style = match self {
            Emphasis::Plain => return,
            Emphasis::Hovered => theme.library_hover,
            Emphasis::Selected => theme.library_cursor,
        };
        for x in area.x..area.x + area.width {
            buf[(x, area.y)].set_style(style);
        }
        if self == Emphasis::Selected {
            buf[(area.x, area.y)]
                .set_char('\u{258c}')
                .set_style(theme.bar);
        }
    }

    /// The style for a row's main text.
    pub fn strong(self, theme: &Theme) -> Style {
        match self {
            Emphasis::Selected => theme.library_cursor,
            Emphasis::Hovered => theme.library_hover,
            Emphasis::Plain => theme.track_normal,
        }
    }

    /// The style for the rest of it.
    pub fn quiet(self, theme: &Theme) -> Style {
        match self {
            Emphasis::Selected => theme.library_cursor,
            _ => theme.hint_desc,
        }
    }

    /// The heart's.
    pub fn heart(self, theme: &Theme) -> Style {
        match self {
            Emphasis::Selected => theme.library_cursor,
            _ => theme.favourite,
        }
    }
}

/// What a row's links lead to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Artist(i64),
    Album(i64),
}

/// The text of one row, put on a list's columns. Returns the links it drew.
pub struct Row<'a> {
    pub favourite: bool,
    pub lead: &'a str,
    pub name: &'a str,
    pub artist: Option<(&'a str, Option<i64>)>,
    pub detail: Option<(&'a str, Option<Target>)>,
    pub tail: &'a str,
}

impl Row<'_> {
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        buf: &mut Buffer,
        cols: &Columns,
        area: Rect,
        theme: &Theme,
        emphasis: Emphasis,
        hover: Option<Target>,
        links: &mut Vec<(Rect, Target)>,
    ) {
        let y = area.y;
        emphasis.ground(buf, area, theme);
        if self.favourite {
            buf.set_string(cols.heart, y, "\u{2665}", emphasis.heart(theme));
        }
        let quiet = emphasis.quiet(theme);
        if cols.lead.w > 0 {
            put_right(
                buf,
                cols.lead.x + cols.lead.w,
                y,
                cols.lead.w,
                self.lead,
                quiet,
            );
        }
        put(
            buf,
            cols.name.x,
            y,
            cols.name.w,
            self.name,
            emphasis.strong(theme),
        );
        let mut linked = |buf: &mut Buffer, at: Span, text: &str, target: Option<Target>| {
            let style = link(quiet, target.is_some() && target == hover);
            let rect = put(buf, at.x, y, at.w, text, style);
            if let Some(target) = target
                && rect.width > 0
            {
                links.push((rect, target));
            }
        };
        if let Some((artist, id)) = self.artist
            && cols.artist.w > 0
        {
            linked(buf, cols.artist, artist, id.map(Target::Artist));
        }
        if let Some((detail, target)) = self.detail
            && cols.detail.w > 0
        {
            linked(buf, cols.detail, detail, target);
        }
        if cols.tail.w > 0 {
            put_right(
                buf,
                cols.tail.x + cols.tail.w,
                y,
                cols.tail.w,
                self.tail,
                quiet,
            );
        }
    }
}

/// A section's heading: its name and how many there are, and, when it leads
/// somewhere, a chevron; a rule runs to the end. Returns the clickable part.
pub fn heading(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    name: &str,
    count: &str,
    opens: bool,
    hovered: bool,
) -> Rect {
    let y = area.y;
    let x = area.x + 1;
    let end = area.x + area.width;
    let style = link(theme.heading.add_modifier(Modifier::BOLD), hovered && opens);
    let a = put(buf, x, y, end.saturating_sub(x), name, style);
    let mut at = a.x + a.width;
    if !count.is_empty() && at + 1 < end {
        let c = put(buf, at + 1, y, end - at - 1, count, theme.hint_desc);
        at = c.x + c.width;
    }
    if opens && at + 2 < end {
        let c = put(buf, at + 1, y, end - at - 1, "\u{203a}", theme.hint_desc);
        at = c.x + c.width;
    }
    let clickable = Rect::new(x, y, at.saturating_sub(x), 1);
    for cx in (at + 1).min(end)..end.saturating_sub(1) {
        buf[(cx, y)].set_char('\u{2500}').set_style(theme.rule);
    }
    clickable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fit_inside_the_row_at_any_width() {
        for w in 0..220u16 {
            let area = Rect::new(2, 0, w, 1);
            let c = Columns::new(area, 3, 5, true, true);
            let end = area.x + area.width;
            for s in [c.name, c.artist, c.detail] {
                assert!(s.w == 0 || s.x + s.w <= c.tail.x.max(s.x), "{w}: {c:?}");
            }
            assert!(
                c.tail.w == 0 || c.tail.x + c.tail.w < end || w < 9,
                "{w}: {c:?}"
            );
        }
    }

    #[test]
    fn rows_of_one_list_share_their_columns() {
        let area = Rect::new(0, 0, 100, 1);
        let a = Columns::new(area, 0, 7, true, true);
        let b = Columns::new(area, 0, 7, true, true);
        assert_eq!(a, b);
        assert!(a.artist.w > 0 && a.detail.w > 0);
    }
}

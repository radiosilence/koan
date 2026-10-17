//! What every page sits in: the gutters around it, the toolbar row above it,
//! text measured and cut to a width, and links to artists and albums.
//!
//! A page does not draw its own heading. It says what its toolbar holds
//! ([`Toolbar`]): a title, a count, and its controls, each of which is a key
//! the page already handles, so a click on one is that key pressed. The shell
//! lays the row out, drops the least important controls when it is narrow,
//! and gives the page the rect below it with the gutters taken off.

use std::cell::RefCell;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

use crate::app::NavTarget;
use crate::text::{fit, width};
use crate::theme::Theme;

/// Rows between the top of the content area and the toolbar.
pub const TOP_GUTTER: u16 = 1;
/// Columns between the content and the sidebar, and the content and the
/// terminal's right edge.
pub const SIDE_GUTTER: u16 = 1;
/// Columns between a toolbar's items.
pub const ITEM_GAP: u16 = 2;

/// `area` with the side gutters taken off, and the top one.
pub fn content(area: Rect) -> Rect {
    let side = SIDE_GUTTER.min(area.width / 2);
    let top = TOP_GUTTER.min(area.height);
    Rect::new(
        area.x + side,
        area.y + top,
        area.width - 2 * side,
        area.height - top,
    )
}

/// `area` with a cell taken off each side: for a panel within a page.
pub fn inset(area: Rect) -> Rect {
    let side = SIDE_GUTTER.min(area.width / 2);
    Rect::new(area.x + side, area.y, area.width - 2 * side, area.height)
}

/// One of a page's controls in the toolbar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolItem {
    pub label: String,
    /// The key it stands for: a click is this key pressed.
    pub key: KeyEvent,
    /// Lit: a toggle that is on, a filter with something typed.
    pub on: bool,
    /// When the row is too narrow, the lowest go first. Ties go from the
    /// left.
    pub priority: u8,
}

impl ToolItem {
    pub fn new(label: impl Into<String>, key: KeyEvent) -> Self {
        Self {
            label: label.into(),
            key,
            on: false,
            priority: 0,
        }
    }

    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    fn cells(&self) -> u16 {
        width(&self.label) as u16
    }
}

/// The frame every modal draws: a plain line in the rule colour, its title
/// lowercase in the heading style at the top left.
pub fn frame(title: &str, theme: &Theme) -> ratatui::widgets::Block<'static> {
    use ratatui::widgets::{Block, Borders};
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme.rule)
        .title(ratatui::text::Span::styled(
            format!(" {title} "),
            theme.heading,
        ))
}

/// Blanks `area` for a modal to draw on, and the halves of any wide
/// character its edges cut through: left half outside, the terminal would
/// draw the character over the modal's border.
pub fn clear(buf: &mut Buffer, area: Rect) {
    use ratatui::widgets::Widget;
    ratatui::widgets::Clear.render(area, buf);
    let bounds = buf.area;
    for y in area.y..area.y + area.height {
        if area.x > bounds.x && width(buf[(area.x - 1, y)].symbol()) > 1 {
            buf[(area.x - 1, y)].reset();
        }
        let right = area.x + area.width;
        if right < bounds.x + bounds.width && buf[(right, y)].symbol().is_empty() {
            buf[(right, y)].reset();
        }
    }
}

/// A modal `w` by `h` cells, centred in `area` and kept inside it.
pub fn centred(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// What a page shows in the toolbar row: its title and count on the left,
/// its controls on the right.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toolbar {
    pub title: String,
    pub count: Option<String>,
    pub items: Vec<ToolItem>,
}

impl Toolbar {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn count(mut self, count: impl Into<String>) -> Self {
        self.count = Some(count.into());
        self
    }

    pub fn item(mut self, item: ToolItem) -> Self {
        self.items.push(item);
        self
    }
}

/// What a click on the toolbar does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolHit {
    Nav(NavTarget),
    Key(KeyEvent),
}

/// The items of `bar` that fit beside the title in `room` cells, in their
/// order, the lowest priorities dropped first.
fn fitting(items: &[ToolItem], room: u16) -> Vec<&ToolItem> {
    let mut kept: Vec<&ToolItem> = items.iter().collect();
    let cost =
        |kept: &[&ToolItem]| -> u16 { kept.iter().map(|i| i.cells() + ITEM_GAP).sum::<u16>() };
    while !kept.is_empty() && cost(&kept) > room {
        let (at, _) = kept
            .iter()
            .enumerate()
            .min_by_key(|(_, i)| i.priority)
            .expect("not empty");
        kept.remove(at);
    }
    kept
}

/// Draws the toolbar into the row `area`: back and forward, the title and the
/// count, and the page's controls flush right. Returns what each part does
/// when clicked. `pointer` lights what it is over.
pub fn draw_toolbar(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    bar: &Toolbar,
    can_back: bool,
    can_forward: bool,
    pointer: (u16, u16),
) -> Vec<(Rect, ToolHit)> {
    let mut hits = Vec::new();
    if area.width == 0 || area.height == 0 {
        return hits;
    }
    let area = Rect::new(area.x, area.y, area.width, 1);
    let end = area.x + area.width;
    let over = |r: Rect| r.contains(Position::new(pointer.0, pointer.1));
    let nav_style = |on: bool, r: Rect| {
        if !on {
            theme.rule
        } else if over(r) {
            theme.heading
        } else {
            theme.track_normal
        }
    };

    // ‹ ›, then the title.
    let mut x = area.x;
    for (glyph, target, on) in [
        ("\u{2039}", NavTarget::Back, can_back),
        ("\u{203a}", NavTarget::Forward, can_forward),
    ] {
        if x + 2 > end {
            return hits;
        }
        let r = Rect::new(x, area.y, 2, 1);
        buf.set_string(x, area.y, glyph, nav_style(on, r));
        hits.push((r, ToolHit::Nav(target)));
        x += 2;
    }
    x += 1;

    let title_w = width(&bar.title) as u16;
    let count = bar.count.as_deref().unwrap_or("");
    let count_w = if count.is_empty() {
        0
    } else {
        width(count) as u16 + 2
    };
    let room = end.saturating_sub(x);
    let items = fitting(&bar.items, room.saturating_sub(title_w + count_w));
    let items_w: u16 = items.iter().map(|i| i.cells() + ITEM_GAP).sum();

    let left_room = room.saturating_sub(items_w) as usize;
    let title = fit(&bar.title, left_room);
    buf.set_stringn(x, area.y, &title, left_room, theme.heading);
    let used = width(&title);
    if !count.is_empty() && used + 2 < left_room {
        let rest = left_room - used - 2;
        buf.set_stringn(
            x + used as u16 + 2,
            area.y,
            fit(count, rest),
            rest,
            theme.hint_desc,
        );
    }

    // The controls, flush right.
    let mut rx = end - items_w;
    for item in items {
        rx += ITEM_GAP;
        let r = Rect::new(rx, area.y, item.cells(), 1);
        let style = if item.on {
            theme.heading.add_modifier(Modifier::BOLD)
        } else if over(r) {
            theme.track_normal
        } else {
            theme.hint_desc
        };
        buf.set_string(rx, area.y, &item.label, style);
        hits.push((r, ToolHit::Key(item.key)));
        rx += item.cells();
    }
    hits
}

/// Where a link leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Artist(i64),
    Album(i64),
    /// The artist of the library track with this id.
    ArtistOfTrack(i64),
    /// The album of the library track with this id.
    AlbumOfTrack(i64),
}

/// The links drawn this frame and where, so a click on one opens it wherever
/// it was drawn: a row, a heading, the transport. Shared by reference while
/// a frame is drawn; [`Links::begin`] starts each frame.
#[derive(Default)]
pub struct Links {
    drawn: RefCell<Vec<(Rect, Link)>>,
    pointer: std::cell::Cell<(u16, u16)>,
}

impl Links {
    /// Forgets the last frame's links. `pointer` is where the mouse is, for
    /// underlining the link under it.
    pub fn begin(&self, pointer: (u16, u16)) {
        self.drawn.borrow_mut().clear();
        self.pointer.set(pointer);
    }

    /// Draws `text` at `x`, `y` in at most `max` cells as a link to `link`,
    /// underlined while the pointer is over it. Returns the cells it took.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        text: &str,
        max: u16,
        style: Style,
        link: Link,
    ) -> u16 {
        let text = fit(text, max as usize);
        let w = width(&text) as u16;
        if w == 0 {
            return 0;
        }
        let r = Rect::new(x, y, w, 1);
        let (px, py) = self.pointer.get();
        let style = if r.contains(Position::new(px, py)) {
            style.add_modifier(Modifier::UNDERLINED)
        } else {
            style
        };
        buf.set_stringn(x, y, &text, max as usize, style);
        self.drawn.borrow_mut().push((r, link));
        w
    }

    /// The link drawn at `col`, `row` this frame.
    pub fn at(&self, col: u16, row: u16) -> Option<Link> {
        self.drawn
            .borrow()
            .iter()
            .find(|(r, _)| r.contains(Position::new(col, row)))
            .map(|(_, l)| *l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn a_modal_never_leaves_half_a_wide_character_at_its_edge() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        buf.set_string(0, 0, "東京事変", Style::default());
        // Columns 3 and 4: the second half of 京 and the first of 事.
        clear(&mut buf, Rect::new(3, 0, 2, 1));
        assert_eq!(buf[(2, 0)].symbol(), " ");
        assert_eq!(buf[(5, 0)].symbol(), " ");
        assert_eq!(buf[(6, 0)].symbol(), "変");
        assert_eq!(buf[(0, 0)].symbol(), "東");
    }

    #[test]
    fn a_modal_stays_inside_its_area() {
        let area = Rect::new(10, 5, 40, 12);
        assert_eq!(centred(area, 20, 6), Rect::new(20, 8, 20, 6));
        assert_eq!(centred(area, 90, 60), area);
    }

    #[test]
    fn content_keeps_the_gutters() {
        let r = content(Rect::new(18, 0, 100, 30));
        assert_eq!(r, Rect::new(19, 1, 98, 29));
        // Nothing to take them from.
        assert_eq!(content(Rect::new(0, 0, 1, 0)), Rect::new(0, 0, 1, 0));
    }

    #[test]
    fn a_narrow_toolbar_drops_its_least_important_controls() {
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let bar = Toolbar::new("albums")
            .count("1,204")
            .item(ToolItem::new("/ filter", key('/')).priority(3))
            .item(ToolItem::new("sort: artist", key('o')).priority(1))
            .item(ToolItem::new("tiles", key('v')).priority(2));
        let theme = Theme::default();
        for width in [20u16, 40, 60, 100] {
            let area = Rect::new(0, 0, width, 1);
            let mut buf = Buffer::empty(area);
            let hits = draw_toolbar(&mut buf, area, &theme, &bar, true, false, (0, 0));
            let mut end = 0;
            for (r, _) in &hits {
                assert!(r.x >= end, "items overlap at width {width}");
                assert!(r.x + r.width <= width, "past the edge at width {width}");
                end = r.x + r.width;
            }
            let keys: Vec<_> = hits
                .iter()
                .filter_map(|(_, h)| match h {
                    ToolHit::Key(k) => Some(k.code),
                    _ => None,
                })
                .collect();
            if keys.contains(&KeyCode::Char('o')) {
                assert!(keys.contains(&KeyCode::Char('/')));
                assert!(keys.contains(&KeyCode::Char('v')));
            }
        }
        let area = Rect::new(0, 0, 100, 1);
        let mut buf = Buffer::empty(area);
        assert_eq!(
            draw_toolbar(&mut buf, area, &theme, &bar, true, true, (0, 0)).len(),
            5
        );
    }

    #[test]
    fn a_link_is_found_where_it_was_drawn() {
        let links = Links::default();
        let area = Rect::new(0, 0, 40, 2);
        let mut buf = Buffer::empty(area);
        links.begin((3, 1));
        let w = links.draw(
            &mut buf,
            2,
            1,
            "Boards of Canada",
            8,
            Style::default(),
            Link::Artist(7),
        );
        assert_eq!(w, 8);
        assert_eq!(links.at(5, 1), Some(Link::Artist(7)));
        assert_eq!(links.at(10, 1), None);
        assert!(buf[(3, 1)].modifier.contains(Modifier::UNDERLINED));
        links.begin((0, 0));
        assert_eq!(links.at(5, 1), None);
    }
}

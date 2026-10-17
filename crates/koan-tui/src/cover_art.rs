use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Capability, Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::protocol::kitty::Kitty;
use ratatui_image::{Image, Resize};

/// A pre-rendered cell: relative position + fg/bg colors.
struct RenderedCell {
    rx: u16,
    ry: u16,
    fg: Color,
    bg: Color,
}

/// Cached rendered output at a specific size. Avoids re-running
/// Lanczos3 resize + pixel iteration every frame.
struct RenderedArt {
    cells: Vec<RenderedCell>,
}

/// Draws images into the buffer, in the terminal's graphics protocol when it
/// has one and the person has not turned it off, otherwise in half blocks.
/// Either way a drawing is made once per image and size and kept, so a frame
/// that shows the same cover again costs a blit.
pub struct Gfx {
    picker: Option<Picker>,
    graphics: bool,
    drawn: HashMap<(u64, u16, u16, u8), (u64, Drawn)>,
    tick: u64,
    /// Kitty image ids no longer drawn, to be deleted from the terminal.
    stale: Vec<u32>,
    next_id: u32,
}

enum Drawn {
    /// With the kitty image id, when it is one the terminal holds.
    Protocol(Protocol, Option<u32>),
    Cells(RenderedArt),
    Unsupported,
}

impl Drawn {
    fn kitty_id(&self) -> Option<u32> {
        match self {
            Self::Protocol(_, id) => *id,
            _ => None,
        }
    }
}

/// Where this run's kitty image ids start, apart from other runs' in the
/// same terminal.
fn seed_id() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.subsec_nanos());
    now ^ std::process::id().rotate_left(16)
}

/// The protocols covers are drawn in. Sixel is left out: the encoder in the
/// release of `ratatui-image` that builds on the MSRV panics at ordinary
/// sizes, so a sixel-only terminal gets half blocks.
fn usable(protocol: ProtocolType) -> bool {
    matches!(protocol, ProtocolType::Kitty | ProtocolType::Iterm2)
}

/// How many drawings are kept. A screenful of tiles, each drawn both as an
/// image and in half blocks, is well under this;
/// scrolling a long library through it is not, so the least recently drawn
/// go first and the tiles in view stay put.
const DRAWN_LIMIT: usize = 384;

impl Default for Gfx {
    fn default() -> Self {
        Self::halfblocks()
    }
}

impl Gfx {
    pub fn halfblocks() -> Self {
        Self {
            picker: None,
            graphics: false,
            drawn: HashMap::new(),
            tick: 0,
            stale: Vec::new(),
            next_id: seed_id(),
        }
    }

    /// Asks the terminal what it can draw. Must run before the event loop
    /// reads the keyboard: the reply arrives on stdin.
    pub fn detect(graphics: bool) -> Self {
        if !graphics {
            return Self::halfblocks();
        }
        let picker = Picker::from_query_stdio_with_options(QueryStdioOptions {
            // Deflating costs CPU on every transmit; it only pays when the
            // link to the terminal is slow.
            kitty_compression: std::env::var_os("SSH_CONNECTION").is_some(),
            ..QueryStdioOptions::default()
        });
        match &picker {
            Ok(p) => log::info!(
                "cover graphics: {:?}, cells {:?}px",
                p.protocol_type(),
                p.font_size()
            ),
            Err(e) => log::warn!("cover graphics: the terminal did not answer: {e}"),
        }
        let picker = picker.ok();
        Self {
            graphics: picker.as_ref().is_some_and(|p| usable(p.protocol_type())),
            picker,
            drawn: HashMap::new(),
            tick: 0,
            stale: Vec::new(),
            next_id: seed_id(),
        }
    }

    /// The half-block pixel's height over its width, against a cell twice as
    /// tall as wide, from the cell size the terminal reported.
    fn squash(&self) -> f32 {
        self.picker
            .as_ref()
            .map(|p| {
                let font = p.font_size();
                2.0 * f32::from(font.width) / f32::from(font.height)
            })
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0)
    }

    /// Whether covers are drawn as real images.
    pub fn graphics(&self) -> bool {
        self.graphics
    }

    /// Whether the terminal reported a graphics protocol at all.
    pub fn available(&self) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|p| usable(p.protocol_type()))
    }

    /// Switches between real images and half blocks. Returns what is now in
    /// effect: off when the terminal has no graphics protocol to switch to.
    pub fn set_graphics(&mut self, on: bool) -> bool {
        let on = on && self.available();
        if on != self.graphics {
            self.graphics = on;
            self.stale
                .extend(self.drawn.drain().filter_map(|(_, (_, d))| d.kitty_id()));
        }
        on
    }

    pub fn protocol_name(&self) -> &'static str {
        match self.picker.as_ref().map(Picker::protocol_type) {
            Some(ProtocolType::Kitty) => "kitty",
            Some(ProtocolType::Iterm2) => "iterm2",
            Some(ProtocolType::Sixel) => "sixel",
            _ => "halfblocks",
        }
    }

    /// Draws `img`, keyed by `key`, into `area`, top-left or centred.
    pub fn draw(
        &mut self,
        key: u64,
        img: &DynamicImage,
        area: Rect,
        buf: &mut Buffer,
        center: bool,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let slot = (key, area.width, area.height, center as u8);
        self.tick += 1;
        if let Some((used, _)) = self.drawn.get_mut(&slot) {
            *used = self.tick;
        } else {
            self.make_room();
            let made = if self.graphics {
                self.protocol(img, area)
            } else {
                Drawn::Cells(pre_render(
                    img,
                    area.width,
                    area.height,
                    center,
                    self.squash(),
                ))
            };
            self.drawn.insert(slot, (self.tick, made));
        }
        match self.drawn.get(&slot).map(|(_, d)| d) {
            Some(Drawn::Protocol(proto, _)) => {
                let size = proto.size();
                let (w, h) = (size.width.min(area.width), size.height.min(area.height));
                let (dx, dy) = if center {
                    ((area.width - w) / 2, (area.height - h) / 2)
                } else {
                    (0, 0)
                };
                let rect = Rect::new(area.x + dx, area.y + dy, w, h);
                Image::new(proto).render(rect, buf);
            }
            Some(Drawn::Cells(rendered)) => blit(rendered, area, area.height, buf),
            _ => {}
        }
    }

    /// Drops the least recently drawn quarter once the limit is reached.
    fn make_room(&mut self) {
        if self.drawn.len() < DRAWN_LIMIT {
            return;
        }
        let mut ticks: Vec<u64> = self.drawn.values().map(|(used, _)| *used).collect();
        let cut = *ticks.select_nth_unstable(DRAWN_LIMIT / 4).1;
        let stale = &mut self.stale;
        self.drawn.retain(|_, (used, drawn)| {
            let keep = *used > cut;
            if !keep {
                stale.extend(drawn.kitty_id());
            }
            keep
        });
    }

    /// A kitty image whose pixels the terminal keeps until told otherwise,
    /// so each evicted cover is deleted there too. Sized to the cell grid and
    /// transmitted once, as `Picker::new_protocol` does, but with an id of
    /// our own to delete by.
    fn protocol(&mut self, img: &DynamicImage, area: Rect) -> Drawn {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        let Some(picker) = self.picker.as_ref() else {
            return Drawn::Unsupported;
        };
        let size = Size::new(area.width, area.height);
        let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
        if picker.protocol_type() != ProtocolType::Kitty {
            return picker
                .new_protocol(img.clone(), size, resize)
                .map_or(Drawn::Unsupported, |p| Drawn::Protocol(p, None));
        }
        let font = picker.font_size();
        let fit = resize.size_for(img, font, size);
        let scaled = resize.resize(img, font, fit, None);
        let compress = picker
            .capabilities()
            .contains(&Capability::KittyCompression);
        Kitty::new(scaled, fit, id, picker.tmux_detected(), compress)
            .map_or(Drawn::Unsupported, |k| {
                Drawn::Protocol(Protocol::Kitty(k), Some(id))
            })
    }

    /// The escape sequences that delete, from the terminal, the covers that
    /// were dropped since the last call. Written after a frame.
    pub fn take_deletes(&mut self) -> Option<String> {
        if self.stale.is_empty() {
            return None;
        }
        let tmux = self.picker.as_ref().is_some_and(Picker::tmux_detected);
        let (start, escape, end) = if tmux {
            ("\x1bPtmux;", "\x1b\x1b", "\x1b\\")
        } else {
            ("", "\x1b", "")
        };
        let out = self
            .stale
            .drain(..)
            .map(|id| format!("{start}{escape}_Ga=d,d=I,i={id},q=2{escape}\\{end}"))
            .collect();
        Some(out)
    }

    /// Queues every cover for deletion, for the exit.
    pub fn release_all(&mut self) {
        self.stale
            .extend(self.drawn.drain().filter_map(|(_, (_, d))| d.kitty_id()));
    }

    /// Draws the top `visible` rows of the image that would fill `area`, for a
    /// tile the page ends part-way through. A graphics protocol cannot be
    /// cropped by a cell, so the part drawn is half blocks. So is a whole tile
    /// while the page is `moving`: kitty draws an image as one placeholder per
    /// cell, and a grid of them moving every frame is more than terminals keep
    /// up with.
    pub fn draw_clipped(
        &mut self,
        key: u64,
        img: &DynamicImage,
        area: Rect,
        visible: u16,
        moving: bool,
        buf: &mut Buffer,
    ) {
        if visible >= area.height && !(moving && self.graphics) {
            return self.draw(key, img, area, buf, true);
        }
        if visible == 0 || area.width == 0 {
            return;
        }
        let slot = (key, area.width, area.height, 3u8);
        self.tick += 1;
        if let Some((used, _)) = self.drawn.get_mut(&slot) {
            *used = self.tick;
        } else {
            self.make_room();
            self.drawn.insert(
                slot,
                (
                    self.tick,
                    Drawn::Cells(pre_render(
                        img,
                        area.width,
                        area.height,
                        true,
                        self.squash(),
                    )),
                ),
            );
        }
        if let Some((_, Drawn::Cells(rendered))) = self.drawn.get(&slot) {
            blit(rendered, area, visible, buf);
        }
    }
}

fn blit(rendered: &RenderedArt, area: Rect, visible: u16, buf: &mut Buffer) {
    for cell in rendered.cells.iter().filter(|c| c.ry < visible) {
        if let Some(c) = buf.cell_mut(ratatui::layout::Position::new(
            area.x + cell.rx,
            area.y + cell.ry,
        )) {
            c.set_char('\u{2580}')
                .set_style(Style::new().fg(cell.fg).bg(cell.bg));
        }
    }
}

/// The key a file's cover is drawn under.
pub fn path_key(path: &Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish() & !(1 << 63)
}

/// The key an album's tile is drawn under. Apart from `path_key`'s by the top
/// bit, so a tile and a cover never share a drawing.
pub fn album_key(album_id: i64) -> u64 {
    (album_id as u64) | (1 << 63)
}

/// Single-entry cache for cover art images: at most ONE decoded image. When a
/// new path is loaded the old image is dropped, keeping memory bounded.
pub struct CoverArtCache {
    path: Option<PathBuf>,
    image: Option<DynamicImage>,
}

impl Default for CoverArtCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverArtCache {
    pub fn new() -> Self {
        Self {
            path: None,
            image: None,
        }
    }

    /// Get cover art for a file. Transparently caches — only reads the file
    /// on first call for a given path. Returns None if no art is embedded.
    pub fn get(&mut self, path: &Path) -> Option<&DynamicImage> {
        if self.path.as_deref() != Some(path) {
            self.path = Some(path.to_path_buf());
            self.image = koan_core::index::folder_art::cover_art(path)
                .and_then(|bytes| image::load_from_memory(&bytes).ok());
        }
        self.image.as_ref()
    }

    /// Peek at the cached image without triggering a load.
    pub fn cached(&self) -> Option<&DynamicImage> {
        self.image.as_ref()
    }

    /// The key the cached image is drawn under.
    pub fn key(&self) -> u64 {
        self.path.as_deref().map_or(0, path_key)
    }

    /// Clear the cache.
    pub fn clear(&mut self) {
        self.path = None;
        self.image = None;
    }

    pub fn render_to(&self, gfx: &mut Gfx, area: Rect, buf: &mut Buffer) {
        if let Some(img) = &self.image {
            gfx.draw(self.key(), img, area, buf, false);
        }
    }

    pub fn render_to_centered(&self, gfx: &mut Gfx, area: Rect, buf: &mut Buffer) {
        if let Some(img) = &self.image {
            gfx.draw(self.key(), img, area, buf, true);
        }
    }
}

/// Pre-render a DynamicImage into halfblock cells at a given cell size.
/// When `center` is true, the image is centered in the area; otherwise top-left aligned.
/// `squash` is how much shorter a half-block pixel is than it is wide: 1.0 for
/// a cell exactly twice as tall as wide, less for a taller cell. With it the
/// half blocks take the shape the terminal's own image would.
fn pre_render(
    img: &DynamicImage,
    width: u16,
    height: u16,
    center: bool,
    squash: f32,
) -> RenderedArt {
    let aspect = img.height() as f32 / img.width().max(1) as f32 * squash;
    let cols = (width as f32)
        .min(f32::from(height) * 2.0 / aspect)
        .max(1.0);
    let rows = (aspect * cols).round().clamp(1.0, f32::from(height) * 2.0);
    let resized = img.resize_exact(
        cols.round() as u32,
        rows as u32,
        image::imageops::FilterType::Lanczos3,
    );
    // Force even pixel height — halfblocks need pixel pairs, odd heights cause
    // a black bar on the last row.
    let rgba = if !resized.height().is_multiple_of(2) {
        let cropped = resized.crop_imm(0, 0, resized.width(), resized.height() - 1);
        cropped.to_rgba8()
    } else {
        resized.to_rgba8()
    };
    let (img_w, img_h) = rgba.dimensions();

    let y_cell_count = img_h / 2;
    let (x_offset, y_offset) = if center {
        (
            (width.saturating_sub(img_w as u16)) / 2,
            (height.saturating_sub(y_cell_count as u16)) / 2,
        )
    } else {
        (0, 0)
    };

    let mut cells = Vec::new();

    for cy in 0..height {
        for cx in 0..width {
            if cx < x_offset
                || cx >= x_offset + img_w as u16
                || cy < y_offset
                || cy >= y_offset + y_cell_count as u16
            {
                continue;
            }

            let px = cx.saturating_sub(x_offset) as u32;
            let top_py = (cy.saturating_sub(y_offset) as u32) * 2;
            let bot_py = top_py + 1;

            let top = rgba.get_pixel(px.min(img_w - 1), top_py.min(img_h - 1));
            // bot_py is always in bounds because we forced even pixel height.
            let bot = *rgba.get_pixel(px.min(img_w - 1), bot_py.min(img_h - 1));

            cells.push(RenderedCell {
                rx: cx,
                ry: cy,
                fg: Color::Rgb(top[0], top[1], top[2]),
                bg: Color::Rgb(bot[0], bot[1], bot[2]),
            });
        }
    }

    RenderedArt { cells }
}

/// Widget that renders a `DynamicImage` using Unicode halfblock characters.
/// Use this for one-shot renders (e.g. track info modal, zoom overlay).
/// For repeated renders at the same size, prefer `CoverArtCache::render_to()`.
pub struct CoverArt<'a> {
    image: &'a DynamicImage,
    center: bool,
}

impl<'a> CoverArt<'a> {
    pub fn new(image: &'a DynamicImage) -> Self {
        Self {
            image,
            center: false,
        }
    }
}

impl Widget for CoverArt<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let rendered = pre_render(self.image, area.width, area.height, self.center, 1.0);
        for cell in &rendered.cells {
            if let Some(c) = buf.cell_mut(ratatui::layout::Position::new(
                area.x + cell.rx,
                area.y + cell.ry,
            )) {
                c.set_char('\u{2580}')
                    .set_style(Style::new().fg(cell.fg).bg(cell.bg));
            }
        }
    }
}

#[cfg(test)]
impl Gfx {
    /// A drawer that uses `protocol`, for tests: no terminal is asked.
    pub fn forced(protocol: ProtocolType) -> Self {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(protocol);
        Self {
            picker: Some(picker),
            graphics: usable(protocol),
            drawn: HashMap::new(),
            tick: 0,
            stale: Vec::new(),
            next_id: seed_id(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(96, 96, |x, y| {
            image::Rgb([(x * 2) as u8, (y * 2) as u8, 128])
        }))
    }

    fn draw_all(gfx: &mut Gfx) {
        let img = picture();
        for (w, h) in [(1u16, 1u16), (2, 1), (8, 4), (24, 12), (60, 8), (5, 30)] {
            for center in [false, true] {
                let area = Rect::new(2, 1, w, h);
                let mut buf = Buffer::empty(Rect::new(0, 0, 80, 40));
                gfx.draw(7, &img, area, &mut buf, center);
            }
            for visible in [0u16, 1, h / 2, h, h + 3] {
                for moving in [false, true] {
                    let area = Rect::new(2, 1, w, h);
                    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 40));
                    gfx.draw_clipped(9, &img, area, visible, moving, &mut buf);
                }
            }
        }
    }

    #[test]
    fn every_protocol_draws_at_any_size_without_panicking() {
        for protocol in [
            ProtocolType::Halfblocks,
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
        ] {
            draw_all(&mut Gfx::forced(protocol));
        }
    }

    #[test]
    fn sixel_is_never_used() {
        let gfx = Gfx::forced(ProtocolType::Sixel);
        assert!(!gfx.graphics() && !gfx.available());
        // And drawing through it is half blocks, at sizes that crash the
        // sixel encoder.
        draw_all(&mut Gfx::forced(ProtocolType::Sixel));
    }

    #[test]
    fn a_graphics_protocol_puts_something_in_the_cells() {
        let img = picture();
        let area = Rect::new(0, 0, 24, 12);
        let mut buf = Buffer::empty(area);
        Gfx::forced(ProtocolType::Kitty).draw(1, &img, area, &mut buf, false);
        assert!(
            (0..area.width).any(|x| buf[(x, 0)].symbol() != " "),
            "the first row of the image is drawn"
        );
    }

    #[test]
    fn graphics_can_be_switched_off_and_back_on() {
        let mut gfx = Gfx::forced(ProtocolType::Kitty);
        assert!(gfx.graphics() && gfx.available());
        assert!(!gfx.set_graphics(false));
        assert!(!gfx.graphics() && gfx.available());
        assert!(gfx.set_graphics(true));
        // A terminal with no protocol cannot be switched on.
        let mut plain = Gfx::halfblocks();
        assert!(!plain.set_graphics(true));
    }

    #[test]
    fn a_clipped_tile_shows_only_the_rows_that_fit() {
        let img = picture();
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 12));
        Gfx::forced(ProtocolType::Kitty).draw_clipped(3, &img, area, 4, false, &mut buf);
        assert!((0..20).any(|x| buf[(x, 3)].symbol() != " "));
        assert!((0..20).all(|x| buf[(x, 4)].symbol() == " "));
    }

    #[test]
    fn a_moving_tile_is_half_blocks_and_a_still_one_an_image() {
        let img = picture();
        let area = Rect::new(0, 0, 20, 10);
        let mut gfx = Gfx::forced(ProtocolType::Kitty);
        let mut moving = Buffer::empty(area);
        gfx.draw_clipped(3, &img, area, 10, true, &mut moving);
        assert!(moving.content.iter().any(|c| c.symbol() == "\u{2580}"));
        let mut still = Buffer::empty(area);
        gfx.draw_clipped(3, &img, area, 10, false, &mut still);
        assert!(still.content.iter().all(|c| c.symbol() != "\u{2580}"));
        assert!(
            still
                .content
                .iter()
                .any(|c| c.symbol().contains('\u{10EEEE}'))
        );
    }

    #[test]
    fn half_blocks_take_the_shape_of_the_terminal_cell() {
        let square =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(300, 300, image::Rgb([200; 3])));
        let rows = |squash| {
            let drawn = pre_render(&square, 22, 11, false, squash);
            drawn.cells.iter().map(|c| c.ry).max().map_or(0, |r| r + 1)
        };
        // A 13x29 px cell: 22 columns are 286 px, which is ten rows of 29.
        assert_eq!(rows(2.0 * 13.0 / 29.0), 10);
        assert_eq!(rows(1.0), 11);
    }
}

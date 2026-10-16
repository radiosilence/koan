use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
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
    drawn: HashMap<(u64, u16, u16, u8), Drawn>,
}

enum Drawn {
    Protocol(Protocol),
    Cells(RenderedArt),
    Unsupported,
}

/// How many drawings are kept before the lot is dropped. A screenful of
/// tiles is well under this; scrolling a long library through it is not.
const DRAWN_LIMIT: usize = 192;

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
        }
    }

    /// Asks the terminal what it can draw. Must run before the event loop
    /// reads the keyboard: the reply arrives on stdin.
    pub fn detect(graphics: bool) -> Self {
        if !graphics {
            return Self::halfblocks();
        }
        let picker = Picker::from_query_stdio().ok();
        Self {
            graphics: picker
                .as_ref()
                .is_some_and(|p| p.protocol_type() != ProtocolType::Halfblocks),
            picker,
            drawn: HashMap::new(),
        }
    }

    /// Whether covers are drawn as real images.
    pub fn graphics(&self) -> bool {
        self.graphics
    }

    /// Whether the terminal reported a graphics protocol at all.
    pub fn available(&self) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|p| p.protocol_type() != ProtocolType::Halfblocks)
    }

    /// Switches between real images and half blocks. Returns what is now in
    /// effect: off when the terminal has no graphics protocol to switch to.
    pub fn set_graphics(&mut self, on: bool) -> bool {
        let on = on && self.available();
        if on != self.graphics {
            self.graphics = on;
            self.drawn.clear();
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
        if !self.drawn.contains_key(&slot) {
            if self.drawn.len() >= DRAWN_LIMIT {
                self.drawn.clear();
            }
            let made = match self.picker.as_ref().filter(|_| self.graphics) {
                Some(picker) => picker
                    .new_protocol(
                        img.clone(),
                        Rect::new(0, 0, area.width, area.height),
                        Resize::Fit(Some(image::imageops::FilterType::Triangle)),
                    )
                    .map_or(Drawn::Unsupported, Drawn::Protocol),
                None => Drawn::Cells(pre_render(img, area.width, area.height, center)),
            };
            self.drawn.insert(slot, made);
        }
        match self.drawn.get(&slot) {
            Some(Drawn::Protocol(proto)) => {
                let size = proto.area();
                let (w, h) = (size.width.min(area.width), size.height.min(area.height));
                let (dx, dy) = if center {
                    ((area.width - w) / 2, (area.height - h) / 2)
                } else {
                    (0, 0)
                };
                Image::new(proto).render(Rect::new(area.x + dx, area.y + dy, w, h), buf);
            }
            Some(Drawn::Cells(rendered)) => blit(rendered, area, area.height, buf),
            _ => {}
        }
    }

    /// Draws the top `visible` rows of the image that would fill `area`, for a
    /// tile the page ends part-way through. A graphics protocol cannot be
    /// cropped by a cell, so the part drawn is half blocks.
    pub fn draw_clipped(
        &mut self,
        key: u64,
        img: &DynamicImage,
        area: Rect,
        visible: u16,
        buf: &mut Buffer,
    ) {
        if visible >= area.height {
            return self.draw(key, img, area, buf, true);
        }
        if visible == 0 || area.width == 0 {
            return;
        }
        let slot = (key, area.width, area.height, 3u8);
        if !self.drawn.contains_key(&slot) {
            if self.drawn.len() >= DRAWN_LIMIT {
                self.drawn.clear();
            }
            self.drawn.insert(
                slot,
                Drawn::Cells(pre_render(img, area.width, area.height, true)),
            );
        }
        if let Some(Drawn::Cells(rendered)) = self.drawn.get(&slot) {
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
fn pre_render(img: &DynamicImage, width: u16, height: u16, center: bool) -> RenderedArt {
    let target_w = width as u32;
    let target_h = (height as u32) * 2;

    let resized = img.resize(target_w, target_h, image::imageops::FilterType::Lanczos3);
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
        let rendered = pre_render(self.image, area.width, area.height, self.center);
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

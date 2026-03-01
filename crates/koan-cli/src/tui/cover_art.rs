use std::path::{Path, PathBuf};

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

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
    width: u16,
    height: u16,
    cells: Vec<RenderedCell>,
}

/// Transparent single-entry cache for cover art images.
/// Caches both the decoded image AND the rendered halfblock output
/// so frames that don't change size are a cheap blit.
pub struct CoverArtCache {
    path: Option<PathBuf>,
    image: Option<DynamicImage>,
    rendered: Option<RenderedArt>,
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
            rendered: None,
        }
    }

    /// Get cover art for a file. Transparently caches — only reads the file
    /// on first call for a given path. Returns None if no art is embedded.
    pub fn get(&mut self, path: &Path) -> Option<&DynamicImage> {
        if self.path.as_deref() != Some(path) {
            self.path = Some(path.to_path_buf());
            self.image = koan_core::index::metadata::extract_cover_art(path)
                .and_then(|bytes| image::load_from_memory(&bytes).ok());
            self.rendered = None; // invalidate render cache on new image
        }
        self.image.as_ref()
    }

    /// Peek at the cached image without triggering a load.
    pub fn cached(&self) -> Option<&DynamicImage> {
        self.image.as_ref()
    }

    /// Compute how many terminal rows the cached image would occupy at a given
    /// column width, preserving aspect ratio. Returns 0 if no image is cached.
    pub fn cell_height_for_width(&self, width: u16) -> u16 {
        let Some(ref img) = self.image else {
            return 0;
        };
        if width == 0 {
            return 0;
        }
        let (iw, ih) = (img.width(), img.height());
        if iw == 0 || ih == 0 {
            return 0;
        }
        // Target pixel height (2 pixels per cell row via halfblocks).
        let target_ph = (width as u32) * ih / iw;
        // Convert pixel rows → cell rows (ceiling).
        (target_ph.div_ceil(2)) as u16
    }

    /// Clear the cache.
    pub fn clear(&mut self) {
        self.path = None;
        self.image = None;
        self.rendered = None;
    }

    /// Render cached art into a buffer. Uses a pre-rendered cell cache
    /// so repeated calls at the same size skip the resize + pixel work.
    pub fn render_to(&mut self, area: Rect, buf: &mut Buffer) {
        let Some(ref img) = self.image else { return };
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Check if we have a cached render at this size.
        let needs_render = self
            .rendered
            .as_ref()
            .is_none_or(|r| r.width != area.width || r.height != area.height);

        if needs_render {
            self.rendered = Some(pre_render(img, area.width, area.height, false));
        }

        // Blit cached cells.
        if let Some(ref rendered) = self.rendered {
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
}

/// Pre-render a DynamicImage into halfblock cells at a given cell size.
/// When `center` is true, the image is centered in the area; otherwise top-left aligned.
fn pre_render(img: &DynamicImage, width: u16, height: u16, center: bool) -> RenderedArt {
    let target_w = width as u32;
    let target_h = (height as u32) * 2;

    let resized = img.resize(target_w, target_h, image::imageops::FilterType::Lanczos3);
    let rgba = resized.to_rgba8();
    let (img_w, img_h) = rgba.dimensions();

    let y_cell_count = img_h.div_ceil(2);
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
            let bot = if bot_py < img_h {
                *rgba.get_pixel(px.min(img_w - 1), bot_py)
            } else {
                image::Rgba([0, 0, 0, 255])
            };

            cells.push(RenderedCell {
                rx: cx,
                ry: cy,
                fg: Color::Rgb(top[0], top[1], top[2]),
                bg: Color::Rgb(bot[0], bot[1], bot[2]),
            });
        }
    }

    RenderedArt {
        width,
        height,
        cells,
    }
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

    pub fn centered(mut self) -> Self {
        self.center = true;
        self
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

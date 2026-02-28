use std::path::{Path, PathBuf};

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

/// Transparent single-entry cache for cover art images.
/// Calling `get()` with a path returns the image if cached,
/// or extracts and decodes it on the spot. Subsequent calls
/// with the same path are free.
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
            self.image = koan_core::index::metadata::extract_cover_art(path)
                .and_then(|bytes| image::load_from_memory(&bytes).ok());
        }
        self.image.as_ref()
    }

    /// Peek at the cached image without triggering a load.
    pub fn cached(&self) -> Option<&DynamicImage> {
        self.image.as_ref()
    }

    /// Clear the cache (e.g. when closing a modal).
    pub fn clear(&mut self) {
        self.path = None;
        self.image = None;
    }
}

/// Widget that renders a `DynamicImage` using Unicode halfblock characters.
/// Each cell displays 2 vertical pixels: top as foreground, bottom as background.
/// Works in any terminal with Unicode + true color support.
pub struct CoverArt<'a> {
    image: &'a DynamicImage,
}

impl<'a> CoverArt<'a> {
    pub fn new(image: &'a DynamicImage) -> Self {
        Self { image }
    }
}

impl Widget for CoverArt<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        render_halfblocks(self.image, area, buf);
    }
}

/// Render a DynamicImage into a buffer region using Unicode halfblock characters.
/// Each cell displays 2 vertical pixels: top pixel as fg, bottom pixel as bg.
fn render_halfblocks(img: &DynamicImage, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let target_w = area.width as u32;
    let target_h = (area.height as u32) * 2; // 2 pixels per cell vertically

    let resized = img.resize(target_w, target_h, image::imageops::FilterType::Lanczos3);
    let rgba = resized.to_rgba8();
    let (img_w, img_h) = rgba.dimensions();

    // Center the image if it's smaller than the area (aspect ratio preserved).
    let x_offset = (area.width.saturating_sub(img_w as u16)) / 2;
    let y_cell_count = img_h.div_ceil(2);
    let y_offset = (area.height.saturating_sub(y_cell_count as u16)) / 2;

    for cy in 0..area.height {
        for cx in 0..area.width {
            let px = cx.saturating_sub(x_offset) as u32;
            let top_py = (cy.saturating_sub(y_offset) as u32) * 2;
            let bot_py = top_py + 1;

            // Outside image bounds → skip.
            if cx < x_offset
                || cx >= x_offset + img_w as u16
                || cy < y_offset
                || cy >= y_offset + y_cell_count as u16
            {
                continue;
            }

            let top = rgba.get_pixel(px.min(img_w - 1), top_py.min(img_h - 1));
            let bot = if bot_py < img_h {
                *rgba.get_pixel(px.min(img_w - 1), bot_py)
            } else {
                image::Rgba([0, 0, 0, 255])
            };

            if let Some(cell) =
                buf.cell_mut(ratatui::layout::Position::new(area.x + cx, area.y + cy))
            {
                cell.set_char('\u{2580}') // ▀ upper half block
                    .set_style(
                        Style::new()
                            .fg(Color::Rgb(top[0], top[1], top[2]))
                            .bg(Color::Rgb(bot[0], bot[1], bot[2])),
                    );
            }
        }
    }
}

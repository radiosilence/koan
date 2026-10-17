use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, Sender};
use image::DynamicImage;
use image::imageops::FilterType;
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

/// A cover in half blocks at one size.
type RenderedArt = Vec<RenderedCell>;

/// Draws images into the buffer, in the terminal's graphics protocol when it
/// has one and the person has not turned it off, otherwise in half blocks.
/// Either way a drawing is made once per image and size and kept, so a frame
/// that shows the same cover again costs a blit.
pub struct Gfx {
    picker: Option<Picker>,
    graphics: bool,
    drawn: HashMap<(u64, u16, u16, u8), (u64, Drawn)>,
    tick: u64,
    /// Images first built this frame, against `FRESH_PER_FRAME`.
    fresh: usize,
    /// Kitty image ids no longer drawn, to be deleted from the terminal.
    stale: Vec<u32>,
    next_id: u32,
}

enum Drawn {
    /// With the kitty image id, when it is one the terminal holds.
    Protocol(Protocol, Option<u32>),
    Cells(RenderedArt),
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

/// How many grid tiles a frame turns from half blocks into images. A grid
/// that stops after a long scroll has every tile to encode and send; spread
/// over frames, none of them stalls.
const FRESH_PER_FRAME: usize = 12;

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
            fresh: 0,
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
            ..Self::halfblocks()
        }
    }

    /// The half-block pixel's height over its width, against a cell twice as
    /// tall as wide, from the cell size the terminal reported.
    pub fn squash(&self) -> f32 {
        self.picker
            .as_ref()
            .map(|p| {
                let font = p.font_size();
                2.0 * f32::from(font.width) / f32::from(font.height)
            })
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0)
    }

    /// How many rows a square cover `cols` wide takes, rounded up as the
    /// terminal's images are.
    pub fn rows_for(&self, cols: u16) -> u16 {
        ((f32::from(cols) * self.squash() / 2.0).ceil() as u16).max(u16::from(cols > 0))
    }

    /// Whether images are drawn as kitty placeholders, one per cell.
    fn placeholders(&self) -> bool {
        self.graphics
            && self
                .picker
                .as_ref()
                .is_some_and(|p| p.protocol_type() == ProtocolType::Kitty)
    }

    /// Starts a frame of the grid.
    pub fn begin_frame(&mut self) {
        self.fresh = 0;
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
            let made = self
                .graphics
                .then(|| self.protocol(img, area))
                .flatten()
                .unwrap_or_else(|| {
                    Drawn::Cells(pre_render(
                        img,
                        area.width,
                        area.height,
                        center,
                        self.squash(),
                    ))
                });
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
            None => {}
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
    /// our own to delete by. `None` where the protocol cannot encode it, and
    /// the cover is drawn in half blocks instead.
    fn protocol(&mut self, img: &DynamicImage, area: Rect) -> Option<Drawn> {
        let picker = self.picker.as_ref()?;
        let font = picker.font_size();
        let img = near(
            img,
            u32::from(area.width) * u32::from(font.width),
            u32::from(area.height) * u32::from(font.height),
        );
        let size = Size::new(area.width, area.height);
        let resize = Resize::Scale(Some(FilterType::Triangle));
        if picker.protocol_type() != ProtocolType::Kitty {
            return picker
                .new_protocol(img.into_owned(), size, resize)
                .ok()
                .map(|p| Drawn::Protocol(p, None));
        }
        let fit = resize.size_for(&img, font, size);
        let scaled = resize.resize(&img, font, fit, None);
        let compress = picker
            .capabilities()
            .contains(&Capability::KittyCompression);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        Kitty::new(scaled, fit, id, picker.tmux_detected(), compress)
            .ok()
            .map(|k| Drawn::Protocol(Protocol::Kitty(k), Some(id)))
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
        let built = self.drawn.contains_key(&(key, area.width, area.height, 1));
        if visible >= area.height
            && !(moving && self.placeholders())
            && (built || !self.graphics || self.fresh < FRESH_PER_FRAME)
        {
            if !built && self.graphics {
                self.fresh += 1;
            }
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
    for cell in rendered.iter().filter(|c| c.ry < visible) {
        if let Some(c) = buf.cell_mut(ratatui::layout::Position::new(
            area.x + cell.rx,
            area.y + cell.ry,
        )) {
            c.set_char('\u{2580}')
                .set_style(Style::new().fg(cell.fg).bg(cell.bg));
        }
    }
}

/// The key an album's tile is drawn under. Apart from a `Cover`'s by the top
/// bit, so a tile and a cover never share a drawing.
pub fn album_key(album_id: i64) -> u64 {
    (album_id as u64) | (1 << 63)
}

/// The longest side a cover is kept at. Past it a sleeve is more pixels than
/// a terminal shows, held in memory for nothing.
const COVER_SIDE: u32 = 2048;

/// A track's cover, read and decoded off the main thread.
pub struct Cover {
    /// What it is drawn under: a hash of its bytes, so the tracks of one
    /// record share a drawing rather than each making its own.
    pub key: u64,
    pub image: DynamicImage,
    /// The accent it gives; see `accent::from_image`.
    pub accent: Option<(u8, u8, u8)>,
}

type Request = (PathBuf, Option<i64>);
type Loader = (Sender<Request>, Receiver<(PathBuf, Option<Cover>)>);

/// The cover of one track at a time, loaded on a thread of its own. The cover
/// shown stays until the next one arrives, so the tracks of a record follow
/// one another without a blank between them.
#[derive(Default)]
pub struct CoverArtCache {
    wanted: Option<PathBuf>,
    /// Whether the file was still arriving when it was last asked for.
    asked_arriving: bool,
    /// Whether the last answer for `wanted` was no cover.
    missed: bool,
    cover: Option<Cover>,
    loader: Option<Loader>,
}

impl CoverArtCache {
    /// Asks for the cover of the file at `path`. `track` is its row, for the
    /// server's art when the file has none, as a download still arriving may
    /// not; it is only asked for when the path is new, or when a file that
    /// gave no cover while `arriving` has since arrived.
    pub fn want(&mut self, path: &Path, arriving: bool, track: impl FnOnce() -> Option<i64>) {
        let again = self.missed && self.asked_arriving && !arriving;
        if self.wanted.as_deref() == Some(path) && !again {
            return;
        }
        self.wanted = Some(path.to_path_buf());
        self.asked_arriving = arriving;
        self.missed = false;
        let (tx, _) = self.loader.get_or_insert_with(spawn_loader);
        tx.send((path.to_path_buf(), track())).ok();
    }

    /// Takes in what the loader finished. Returns whether the cover changed.
    pub fn poll(&mut self) -> bool {
        let Some((_, rx)) = &self.loader else {
            return false;
        };
        let mut changed = false;
        while let Ok((path, cover)) = rx.try_recv() {
            if self.wanted.as_ref() == Some(&path) {
                self.missed = cover.is_none();
                self.cover = cover;
                changed = true;
            }
        }
        changed
    }

    pub fn cover(&self) -> Option<&Cover> {
        self.cover.as_ref()
    }

    pub fn cached(&self) -> Option<&DynamicImage> {
        self.cover.as_ref().map(|c| &c.image)
    }

    pub fn clear(&mut self) {
        self.wanted = None;
        self.missed = false;
        self.cover = None;
    }

    pub fn render_to(&self, gfx: &mut Gfx, area: Rect, buf: &mut Buffer) {
        if let Some(c) = &self.cover {
            gfx.draw(c.key, &c.image, area, buf, false);
        }
    }

    pub fn render_to_centered(&self, gfx: &mut Gfx, area: Rect, buf: &mut Buffer) {
        if let Some(c) = &self.cover {
            gfx.draw(c.key, &c.image, area, buf, true);
        }
    }
}

/// A thread that loads the latest cover asked for, passing over any asked
/// for since that nobody wants any more. It ends with its cache.
fn spawn_loader() -> Loader {
    let (tx, req) = crossbeam_channel::unbounded::<Request>();
    let (done, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("koan-cover".into())
        .spawn(move || {
            while let Ok(mut next) = req.recv() {
                while let Ok(later) = req.try_recv() {
                    next = later;
                }
                let cover = load_cover(&next.0, next.1);
                if done.send((next.0, cover)).is_err() {
                    break;
                }
            }
        })
        .ok();
    (tx, rx)
}

fn load_cover(path: &Path, track: Option<i64>) -> Option<Cover> {
    let bytes =
        koan_core::index::folder_art::cover_art(path).or_else(|| track.and_then(server_cover))?;
    let mut image = image::load_from_memory(&bytes).ok()?;
    if image.width().max(image.height()) > COVER_SIDE {
        image = image.resize(COVER_SIDE, COVER_SIDE, FilterType::Lanczos3);
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    Some(Cover {
        key: h.finish() & !(1 << 63),
        accent: super::accent::from_image(&image),
        image,
    })
}

/// The server's cover for a track the library has from one.
fn server_cover(track: i64) -> Option<Vec<u8>> {
    let remote_id = {
        let db = koan_core::db::pool::shared().get().ok()?;
        koan_core::db::queries::get_track_row(&db.conn, track)
            .ok()??
            .remote_id?
    };
    let cfg = koan_core::config::Config::cached();
    if !cfg.remote.enabled {
        return None;
    }
    let client = koan_core::helpers::subsonic_client(&cfg)?;
    client
        .get_cover_art(&remote_id, Some(COVER_SIDE))
        .ok()
        .filter(|b| !b.is_empty())
}

/// `img` brought down to about twice `w`×`h` pixels where it is larger, so
/// the filter that makes a drawing reads a few times the pixels it writes
/// rather than the whole sleeve.
fn near(img: &DynamicImage, w: u32, h: u32) -> Cow<'_, DynamicImage> {
    let (w, h) = (w.max(1) * 2, h.max(1) * 2);
    if img.width() > w || img.height() > h {
        Cow::Owned(img.thumbnail(w, h))
    } else {
        Cow::Borrowed(img)
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
    // Whole cells, rounded up as the terminal rounds its own images.
    let rows = ((aspect * cols / 2.0).ceil() * 2.0).clamp(2.0, f32::from(height) * 2.0);
    let (cols, rows) = (cols.round() as u32, rows as u32);
    // `rows` is even, so every cell has its pair of pixels.
    let rgba = near(img, cols, rows)
        .resize_exact(cols, rows, FilterType::Lanczos3)
        .to_rgba8();
    let (img_w, img_h) = (rgba.width() as u16, (rgba.height() / 2) as u16);
    let (x_offset, y_offset) = if center {
        (
            width.saturating_sub(img_w) / 2,
            height.saturating_sub(img_h) / 2,
        )
    } else {
        (0, 0)
    };
    let mut cells = Vec::with_capacity(usize::from(img_w) * usize::from(img_h));
    for cy in 0..img_h.min(height) {
        for cx in 0..img_w.min(width) {
            let top = rgba.get_pixel(u32::from(cx), u32::from(cy) * 2);
            let bot = rgba.get_pixel(u32::from(cx), u32::from(cy) * 2 + 1);
            cells.push(RenderedCell {
                rx: cx + x_offset,
                ry: cy + y_offset,
                fg: Color::Rgb(top[0], top[1], top[2]),
                bg: Color::Rgb(bot[0], bot[1], bot[2]),
            });
        }
    }
    cells
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
            ..Self::halfblocks()
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
        let rows = |cols, squash| {
            let drawn = pre_render(&square, cols, cols, false, squash);
            drawn.iter().map(|c| c.ry).max().map_or(0, |r| r + 1)
        };
        // A 13x29 px cell: 22 columns are 286 px, 9.9 rows of 29, and 30 are
        // 390 px, 13.4 rows; the terminal rounds both up.
        let cell = 2.0 * 13.0 / 29.0;
        assert_eq!(rows(22, cell), 10);
        assert_eq!(rows(30, cell), 14);
        assert_eq!(rows(22, 1.0), 11);
    }

    fn is_image(buf: &Buffer) -> bool {
        buf.content
            .iter()
            .any(|c| c.symbol().contains('\u{10EEEE}'))
    }

    #[test]
    fn a_frame_turns_only_so_many_tiles_into_images() {
        let img = picture();
        let area = Rect::new(0, 0, 8, 4);
        let mut gfx = Gfx::forced(ProtocolType::Kitty);
        let frame = |gfx: &mut Gfx| {
            gfx.begin_frame();
            (0..20)
                .filter(|&key| {
                    let mut buf = Buffer::empty(area);
                    gfx.draw_clipped(key, &img, area, 4, false, &mut buf);
                    is_image(&buf)
                })
                .count()
        };
        assert_eq!(frame(&mut gfx), FRESH_PER_FRAME);
        assert_eq!(frame(&mut gfx), 20);
    }

    #[test]
    fn only_kitty_tiles_turn_to_half_blocks_while_moving() {
        let img = picture();
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        Gfx::forced(ProtocolType::Iterm2).draw_clipped(3, &img, area, 10, true, &mut buf);
        assert!(buf.content.iter().all(|c| c.symbol() != "\u{2580}"));
    }

    /// Polls until the loader has answered, as frames would.
    fn settle(cache: &mut CoverArtCache) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !cache.poll() {
            assert!(
                std::time::Instant::now() < until,
                "the loader never answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn the_tracks_of_a_record_share_one_drawing() {
        let dir = tempfile::tempdir().unwrap();
        picture().save(dir.path().join("cover.png")).unwrap();
        let (a, b) = (dir.path().join("01.flac"), dir.path().join("02.flac"));
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        let mut cache = CoverArtCache::default();
        cache.want(&a, false, || None);
        settle(&mut cache);
        let first = cache.cover().expect("the folder's cover").key;
        cache.want(&b, false, || None);
        // The first record's cover stays up while the second's loads.
        assert_eq!(cache.cover().map(|c| c.key), Some(first));
        settle(&mut cache);
        assert_eq!(cache.cover().map(|c| c.key), Some(first));
        assert_eq!(first >> 63, 0, "apart from the album tiles' keys");
    }

    #[test]
    fn a_file_with_no_cover_while_arriving_is_asked_again_once_it_has() {
        let dir = tempfile::tempdir().unwrap();
        let track = dir.path().join("01.flac");
        std::fs::write(&track, b"x").unwrap();
        let mut cache = CoverArtCache::default();
        cache.want(&track, true, || None);
        settle(&mut cache);
        assert!(cache.cover().is_none());
        picture().save(dir.path().join("cover.png")).unwrap();
        cache.want(&track, false, || None);
        settle(&mut cache);
        assert!(cache.cover().is_some(), "asked again once it arrived");
    }

    #[test]
    fn a_cover_that_arrives_after_the_track_ended_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        picture().save(dir.path().join("cover.png")).unwrap();
        let track = dir.path().join("01.flac");
        std::fs::write(&track, b"x").unwrap();
        let mut cache = CoverArtCache::default();
        cache.want(&track, false, || None);
        cache.clear();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!cache.poll());
        assert!(cache.cover().is_none());
    }
}

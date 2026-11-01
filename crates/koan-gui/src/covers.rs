//! Record sleeves, fetched once per record and size and kept for the session.
//!
//! Keyed by album rather than track: every row of a record shares one image.
//! A view that draws sleeves observes `Covers` and is redrawn as they land.
//! Each is decoded to the renderer's pixel format on the background executor,
//! so drawing one is a texture upload and never a decode on the main thread.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use koan_kit::raw::prelude::*;
use koan_kit::raw::*;

use crate::engine::engine;

enum Held {
    /// Asked for and waiting its turn, or being fetched.
    Loading,
    /// Decoded, with when it was last drawn, by `Covers::tick`.
    Ready(Arc<RenderImage>, Cell<u64>),
    /// The record has no cover, or it would not decode.
    Missing,
    /// The engine failed to fetch it, a server out of reach say: asked
    /// again when drawn after `RETRY`.
    Failed(Instant),
}

/// How many sleeves are fetched at once. Each fetch holds one of the
/// engine's blocking threads and, on a remote library, an HTTP request.
const IN_FLIGHT: usize = 4;
/// How many asks wait their turn. A list scrolled fast asks for more than
/// it shows; past this the oldest asks are forgotten, and asked again if
/// their rows are drawn again.
const WAITING: usize = 48;
/// How many decoded sleeves are kept. Past this the one drawn longest ago
/// is dropped, its texture with it, and fetched again if drawn again.
const KEPT: usize = 400;
const RETRY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Covers {
    held: HashMap<(i64, u32), Held>,
    /// Newest first: what is on screen now goes before what scrolled past.
    waiting: VecDeque<(i64, u32)>,
    running: usize,
    ready: usize,
    tick: Cell<u64>,
    /// Moves when every sleeve is dropped, so a fetch begun before lands
    /// nowhere.
    generation: u64,
}

#[derive(Clone)]
pub struct CoversHandle(pub Entity<Covers>);

impl Global for CoversHandle {}

impl Covers {
    pub fn install(cx: &mut App) {
        let covers = cx.new(|_| Covers::default());
        cx.set_global(CoversHandle(covers));
    }

    /// The sleeve, if it has landed; asks for it otherwise.
    fn get(
        this: &Entity<Self>,
        album_id: i64,
        size: u32,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        let key = (album_id, size);
        let covers = this.read(cx);
        match covers.held.get(&key) {
            Some(Held::Ready(image, drawn)) => {
                let tick = covers.tick.get() + 1;
                covers.tick.set(tick);
                drawn.set(tick);
                return Some(image.clone());
            }
            Some(Held::Loading | Held::Missing) => return None,
            Some(Held::Failed(at)) if at.elapsed() < RETRY => return None,
            Some(Held::Failed(_)) | None => {}
        }
        this.update(cx, |covers, cx| {
            covers.held.insert(key, Held::Loading);
            covers.waiting.push_front(key);
            if covers.waiting.len() > WAITING
                && let Some(forgotten) = covers.waiting.pop_back()
            {
                covers.held.remove(&forgotten);
            }
            covers.pump(cx);
        });
        None
    }

    /// Drops every sleeve: the library's ids now name other records, as
    /// after the index is rebuilt, which hands them out from 1 again.
    pub fn purge(cx: &mut App) {
        let Some(CoversHandle(covers)) = cx.try_global::<CoversHandle>().cloned() else {
            return;
        };
        covers.update(cx, |covers, cx| {
            covers.generation += 1;
            covers.waiting.clear();
            covers.ready = 0;
            for (_, held) in covers.held.drain() {
                if let Held::Ready(image, _) = held {
                    cx.drop_image(image, None);
                }
            }
            cx.notify();
        });
    }

    /// Drops the sleeve drawn longest ago, and its texture, once more than
    /// `KEPT` are held.
    fn evict(&mut self, cx: &mut Context<Self>) {
        if self.ready <= KEPT {
            return;
        }
        let oldest = self
            .held
            .iter()
            .filter_map(|(key, held)| match held {
                // Held elsewhere too: a cached view showing it, which may
                // not draw again for a while. Kept until it lets go.
                Held::Ready(image, _) if Arc::strong_count(image) > 1 => None,
                Held::Ready(_, drawn) => Some((drawn.get(), *key)),
                _ => None,
            })
            .min();
        if let Some((_, key)) = oldest
            && let Some(Held::Ready(image, _)) = self.held.remove(&key)
        {
            self.ready -= 1;
            cx.drop_image(image, None);
        }
    }

    /// Starts the newest waiting fetches while there is room.
    fn pump(&mut self, cx: &mut Context<Self>) {
        while self.running < IN_FLIGHT {
            let Some((album_id, size)) = self.waiting.pop_front() else {
                return;
            };
            self.running += 1;
            let fetch = engine(cx).album_cover_art(album_id, Some(size));
            let renderer = cx.svg_renderer();
            let background = cx.background_executor().clone();
            let generation = self.generation;
            cx.spawn(async move |this, cx| {
                let held = background
                    .spawn(async move {
                        let art = match fetch.await {
                            Ok(Some(art)) => art,
                            Ok(None) => return Held::Missing,
                            Err(e) => {
                                log::debug!("cover for album {album_id}: {e}");
                                return Held::Failed(Instant::now());
                            }
                        };
                        match Image::from_bytes(format(&art.mime), art.data).to_image_data(renderer)
                        {
                            Ok(image) => Held::Ready(image, Cell::new(0)),
                            Err(e) => {
                                log::debug!("cover for album {album_id}: {e}");
                                Held::Missing
                            }
                        }
                    })
                    .await;
                this.update(cx, |covers, cx| {
                    covers.running -= 1;
                    if covers.generation != generation {
                        if let Held::Ready(image, _) = held {
                            cx.drop_image(image, None);
                        }
                        covers.pump(cx);
                        return;
                    }
                    // Only a sleeve that landed changes what is drawn; a miss
                    // leaves the placeholder that was already there.
                    let landed = matches!(held, Held::Ready(..));
                    let failed = matches!(held, Held::Failed(_));
                    if landed {
                        covers.ready += 1;
                        covers.evict(cx);
                    }
                    covers.held.insert((album_id, size), held);
                    covers.pump(cx);
                    if landed {
                        cx.notify();
                    }
                    // Drawn again once the wait is up, so a view that does
                    // not redraw on its own asks for the sleeve again.
                    if failed {
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(RETRY).await;
                            this.update(cx, |_, cx| cx.notify()).ok();
                        })
                        .detach();
                    }
                })
                .ok();
            })
            .detach();
        }
    }
}

fn format(mime: &str) -> ImageFormat {
    match mime {
        "image/png" => ImageFormat::Png,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/bmp" => ImageFormat::Bmp,
        _ => ImageFormat::Jpeg,
    }
}

/// The sleeve for `album_id` drawn at `side` points, if it has landed; asks
/// for it otherwise. Fetched at twice the side, for Retina displays. A view
/// that keeps the image while it shows it keeps it from being evicted.
pub fn image(album_id: Option<i64>, side: Pixels, cx: &mut App) -> Option<Arc<RenderImage>> {
    let covers = cx.global::<CoversHandle>().0.clone();
    Covers::get(&covers, album_id?, (f32::from(side) * 2.0) as u32, cx)
}

/// A square sleeve of `side` points, or the ground's `surface` while there is
/// none.
pub fn draw(image: Option<Arc<RenderImage>>, side: Pixels, surface: Hsla) -> AnyElement {
    match image {
        Some(image) => img(image).size(side).flex_shrink_0().into_any_element(),
        None => div()
            .size(side)
            .flex_shrink_0()
            .bg(surface)
            .into_any_element(),
    }
}

/// `image` and `draw` together, for rows: drawn whenever they are on screen,
/// so their sleeves stay recently used without being held.
pub fn sleeve(album_id: Option<i64>, side: Pixels, surface: Hsla, cx: &mut App) -> AnyElement {
    let image = image(album_id, side, cx);
    draw(image, side, surface)
}

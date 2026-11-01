//! Record sleeves, fetched once per record and size and kept for the session.
//!
//! Keyed by album rather than track: every row of a record shares one image.
//! A view that draws sleeves observes `Covers` and is redrawn as they land.
//! Each is decoded to the renderer's pixel format on the background executor,
//! so drawing one is a texture upload and never a decode on the main thread.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::engine::engine;
use crate::theme;

enum Held {
    /// Asked for and waiting its turn, or being fetched.
    Loading,
    Ready(Arc<RenderImage>),
    Missing,
}

/// How many sleeves are fetched at once. Each fetch holds one of the
/// engine's blocking threads and, on a remote library, an HTTP request.
const IN_FLIGHT: usize = 4;
/// How many asks wait their turn. A list scrolled fast asks for more than
/// it shows; past this the oldest asks are forgotten, and asked again if
/// their rows are drawn again.
const WAITING: usize = 48;

#[derive(Default)]
pub struct Covers {
    held: HashMap<(i64, u32), Held>,
    /// Newest first: what is on screen now goes before what scrolled past.
    waiting: VecDeque<(i64, u32)>,
    running: usize,
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
        match this.read(cx).held.get(&key) {
            Some(Held::Ready(image)) => return Some(image.clone()),
            Some(Held::Loading | Held::Missing) => return None,
            None => {}
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
            cx.spawn(async move |this, cx| {
                let decoded = background
                    .spawn(async move {
                        let art = fetch.await.map_err(|e| e.to_string())?.ok_or("no cover")?;
                        Image::from_bytes(format(&art.mime), art.data)
                            .to_image_data(renderer)
                            .map_err(|e| e.to_string())
                    })
                    .await;
                let held = match decoded {
                    Ok(image) => Held::Ready(image),
                    Err(e) => {
                        log::debug!("cover for album {album_id}: {e}");
                        Held::Missing
                    }
                };
                this.update(cx, |covers, cx| {
                    covers.running -= 1;
                    covers.held.insert((album_id, size), held);
                    covers.pump(cx);
                    cx.notify();
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

/// A square sleeve of `side` points, or the ground's `surface` while there is
/// none. Fetched at twice the side, for Retina displays.
pub fn sleeve(album_id: Option<i64>, side: Pixels, cx: &mut App) -> AnyElement {
    let p = theme::palette(cx);
    let image = album_id.and_then(|id| {
        let covers = cx.global::<CoversHandle>().0.clone();
        Covers::get(&covers, id, (f32::from(side) * 2.0) as u32, cx)
    });
    match image {
        Some(image) => img(image).size(side).flex_shrink_0().into_any_element(),
        None => div()
            .size(side)
            .flex_shrink_0()
            .bg(p.surface)
            .into_any_element(),
    }
}

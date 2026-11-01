//! Record sleeves, fetched once per record and size and kept for the session.
//!
//! Keyed by album rather than track: every row of a record shares one image.
//! A view that draws sleeves observes `Covers` and is redrawn as they land.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::engine::engine;
use crate::theme;

enum Held {
    Loading,
    Ready(Arc<Image>),
    Missing,
}

#[derive(Default)]
pub struct Covers {
    held: HashMap<(i64, u32), Held>,
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
    fn get(this: &Entity<Self>, album_id: i64, size: u32, cx: &mut App) -> Option<Arc<Image>> {
        match this.read(cx).held.get(&(album_id, size)) {
            Some(Held::Ready(image)) => return Some(image.clone()),
            Some(Held::Loading | Held::Missing) => return None,
            None => {}
        }
        this.update(cx, |covers, _| {
            covers.held.insert((album_id, size), Held::Loading)
        });
        let fetch = engine(cx).album_cover_art(album_id, Some(size));
        let this = this.downgrade();
        cx.spawn(async move |cx| {
            let held = match fetch.await {
                Ok(Some(art)) => {
                    Held::Ready(Arc::new(Image::from_bytes(format(&art.mime), art.data)))
                }
                Ok(None) => Held::Missing,
                Err(e) => {
                    log::info!("cover for album {album_id}: {e}");
                    Held::Missing
                }
            };
            this.update(cx, |covers, cx| {
                covers.held.insert((album_id, size), held);
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
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

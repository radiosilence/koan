//! The library: the album, artist and track browsers, and the pages of a
//! record and an artist.

pub mod actions;
pub mod albums;
pub mod browse;
pub mod chrome;
pub mod covers;
pub mod drag;
pub mod feed;
pub mod nav;
pub mod selection;
pub mod tile;
pub mod tokens;
pub mod window;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;
use koan_ffi::KoanEngine;

/// What every library page reads and acts through.
#[derive(Clone)]
pub struct Lib {
    pub engine: Arc<KoanEngine>,
    pub feed: Entity<feed::Feed>,
    pub covers: Entity<covers::Covers>,
    pub nav: Entity<nav::Nav>,
    pub browse: Entity<browse::Browse>,
}

impl Lib {
    /// `browse` is read from disk by the caller, off the main thread.
    pub fn new(engine: Arc<KoanEngine>, browse: browse::Browse, cx: &mut App) -> Self {
        let feed = cx.new(|cx| feed::Feed::new(engine.clone(), cx));
        let covers = cx.new(|cx| covers::Covers::new(engine.clone(), cx));
        let nav = cx.new(|_| nav::Nav::default());
        let browse = cx.new(|_| browse);

        // The artwork follows the library and the connection: a change to
        // the library may have deleted records whose ids come back as others,
        // and a server coming back may answer what it failed to.
        let seen = Rc::new(Cell::new((None, 0)));
        let watched = covers.clone();
        cx.observe(&feed, move |feed, cx| {
            let (version, epoch) = {
                let feed = feed.read(cx);
                (feed.library_version, feed.connection_epoch)
            };
            let (last_version, last_epoch) = seen.replace((version, epoch));
            watched.update(cx, |covers, cx| {
                if version != last_version && last_version.is_some() {
                    covers.sweep(cx);
                }
                if epoch != last_epoch {
                    covers.retry_failed();
                }
            });
        })
        .detach();

        Self {
            engine,
            feed,
            covers,
            nav,
            browse,
        }
    }
}

gpui_kit::actions!(
    library,
    [
        /// Tick everything the page shows.
        SelectAll,
        /// Leave the selection mode, or close what is open.
        Dismiss,
        GoBack,
        GoForward,
    ]
);

/// The library's keys, in the `Library` context every page sets.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-a", SelectAll, Some("Library")),
        KeyBinding::new("escape", Dismiss, Some("Library")),
        KeyBinding::new("secondary-[", GoBack, Some("Library")),
        KeyBinding::new("secondary-]", GoForward, Some("Library")),
    ]);
}

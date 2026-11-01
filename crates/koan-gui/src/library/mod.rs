//! The library: the album, artist and track browsers, and the pages of a
//! record and an artist.

pub mod actions;
pub mod activity;
pub mod album_page;
pub mod albums;
pub mod artist_page;
pub mod artists;
pub mod bench;
pub mod browse;
pub mod chrome;
pub mod covers;
pub mod drag;
pub mod feed;
pub mod format;
pub mod info;
pub mod nav;
pub mod rows;
pub mod selection;
pub mod stage;
pub mod tile;
pub mod tokens;
pub mod tracks;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use koan_ffi::KoanEngine;
use koan_kit::raw::*;

/// A listener on `view` that outlives the borrow of its context: what
/// `cx.listener` gives, for a handler made inside a closure over that context.
pub fn listen<V: 'static, E: ?Sized + 'static>(
    view: WeakEntity<V>,
    f: impl Fn(&mut V, &E, &mut Window, &mut Context<V>) + 'static,
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    move |event, window, cx| {
        view.update(cx, |v, cx| f(v, event, window, cx)).ok();
    }
}

/// What every library page reads and acts through.
#[derive(Clone)]
pub struct Lib {
    pub engine: Arc<KoanEngine>,
    pub feed: Entity<feed::Feed>,
    pub covers: Entity<covers::Covers>,
    pub nav: Entity<nav::Nav>,
    pub browse: Entity<browse::Browse>,
    /// Set while the album grid's benchmark counts its tiles.
    pub bench: Option<bench::Shared>,
}

impl Lib {
    /// `browse` is read from disk by the caller, off the main thread;
    /// `navigator` is the window's.
    pub fn new(
        engine: Arc<KoanEngine>,
        browse: browse::Browse,
        navigator: Entity<crate::navigator::Navigator>,
        bench: Option<bench::Shared>,
        cx: &mut App,
    ) -> Self {
        let feed = cx.new(|cx| feed::Feed::new(engine.clone(), cx));
        let covers = cx.new(|cx| covers::Covers::new(engine.clone(), cx));
        let nav = cx.new(|cx| nav::Nav::new(navigator, cx));
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
            bench,
        }
    }
}

koan_kit::raw::actions!(
    library,
    [
        /// Tick everything the page shows.
        SelectAll,
        /// Leave the selection mode, or close what is open.
        Dismiss,
    ]
);

/// The library's keys, in the `Library` context every page sets: ⌘ on macOS,
/// Ctrl elsewhere.
pub fn bind_keys(cx: &mut App) {
    #[cfg(target_os = "macos")]
    let all = "cmd-a";
    #[cfg(not(target_os = "macos"))]
    let all = "ctrl-a";
    cx.bind_keys([
        KeyBinding::new(all, SelectAll, Some("Library")),
        KeyBinding::new("escape", Dismiss, Some("Library")),
    ]);
}

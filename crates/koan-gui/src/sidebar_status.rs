//! The sidebar's foot, above the activity rows: what this device plays from
//! (sharing its own library, connected to a server, or its own library
//! alone) and how much the library holds, as the Mac's sidebar shows it.

use koan_ffi::Stats;
// The entity, its context and Render are GPUI's app model, which koan-kit does not wrap.
use koan_kit::app::*;

use crate::components::StatusBlock;
use crate::engine::engine;
use crate::library::chrome::count;
use crate::mirror::Mirror;

pub struct SidebarStatus {
    stats: Option<Stats>,
    /// A read in flight, and whether the library moved meanwhile: a scan
    /// moves it with every batch, and the counts are read again once, when
    /// the read ends, rather than once per batch.
    reading: bool,
    stale: bool,
    _subscriptions: Vec<Subscription>,
}

impl SidebarStatus {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let sharing = cx.global::<crate::settings::Settings>().sharing.clone();
        let mut status = Self {
            stats: None,
            reading: false,
            stale: false,
            _subscriptions: vec![
                cx.observe(&mirror.library, |this, _, cx| this.read(cx)),
                cx.observe(&mirror.cache, |this, _, cx| this.read(cx)),
                cx.observe(&mirror.connection, |_, _, cx| cx.notify()),
                cx.observe(&sharing, |_, _, cx| cx.notify()),
            ],
        };
        status.read(cx);
        status
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            self.stale = true;
            return;
        }
        self.reading = true;
        let stats = engine(cx).library_stats();
        cx.spawn(async move |this, cx| {
            let stats = stats.await;
            this.update(cx, |this, cx| {
                this.reading = false;
                match stats {
                    Ok(stats) => this.stats = Some(stats),
                    Err(e) => log::warn!("library stats: {e}"),
                }
                if std::mem::take(&mut this.stale) {
                    this.read(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for SidebarStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Settings' words for what the device is doing, a failed share
        // included.
        let mode = crate::settings::panes::server::mode_label(
            cx.global::<crate::settings::Settings>().sharing.read(cx),
            &cx.global::<Mirror>().connection.read(cx).0,
        );
        let (icon, text, tone) = mode.line();
        let mut block = StatusBlock::new().line(icon, text, tone);
        if let Some(s) = &self.stats {
            let n = |v: i64| v.max(0) as u64;
            block = block
                .figure(count(n(s.total_tracks), "track", "tracks"))
                .figure(format!(
                    "{} · {}",
                    count(n(s.total_albums), "album", "albums"),
                    count(n(s.total_artists), "artist", "artists")
                ));
            if s.remote_tracks > 0 {
                block = block.figure(format!(
                    "{} of {} remote cached",
                    count(n(s.cached_tracks), "", "").trim(),
                    count(n(s.remote_tracks), "", "").trim()
                ));
            }
        }
        block
    }
}

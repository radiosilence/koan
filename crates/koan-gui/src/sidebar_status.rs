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
use crate::theme::{Icon, Tone};

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
                // Read once a scan or sync ends, if the library moved under it.
                cx.observe(&mirror.tasks, |this, _, cx| {
                    if this.stale && !this.reading {
                        this.stale = false;
                        this.read(cx);
                    }
                }),
                cx.observe(&mirror.connection, |_, _, cx| cx.notify()),
                cx.observe(&sharing, |_, _, cx| cx.notify()),
            ],
        };
        status.read(cx);
        status
    }

    /// Counts the library again: one read at a time, and none while a scan
    /// or sync is moving it, which bumps the library every batch; the
    /// figures are read once that ends.
    fn read(&mut self, cx: &mut Context<Self>) {
        let tasks = cx.global::<Mirror>().tasks.read(cx).0;
        if self.reading || tasks.scanning || tasks.syncing {
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

/// A server's address as people say it: its host.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', ':']).next().unwrap_or(rest)
}

impl Render for SidebarStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let serving = cx
            .global::<crate::settings::Settings>()
            .sharing
            .read(cx)
            .serving
            .as_ref()
            .map(|s| s.port);
        let server = cx
            .global::<Mirror>()
            .connection
            .read(cx)
            .0
            .as_ref()
            .and_then(|c| c.signed_in_to.clone());
        let mut block = StatusBlock::new();
        block = match (serving, server) {
            (Some(port), _) => block.link(
                Icon::Server,
                format!("sharing on port {port}"),
                Tone::Muted,
                "Server settings",
                |_, cx| crate::settings::open_server(cx),
            ),
            (None, Some(server)) => block.link(
                Icon::Server,
                format!("connected to {}", host(&server)),
                Tone::Muted,
                "Server settings",
                |_, cx| crate::settings::open_server(cx),
            ),
            (None, None) => block.line(
                Icon::Library,
                "playing this computer's own library",
                Tone::Muted,
            ),
        };
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

#[cfg(test)]
mod tests {
    use super::host;
    use core::prelude::v1::test;

    #[test]
    fn a_server_is_named_by_its_host() {
        assert_eq!(host("https://koan.blit.cc"), "koan.blit.cc");
        assert_eq!(host("http://192.168.1.20:4533/"), "192.168.1.20");
        assert_eq!(host("koan.local"), "koan.local");
    }
}

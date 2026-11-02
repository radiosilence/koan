//! What the library is busy with, as rows at the foot of the sidebar: a scan,
//! a rebuild of the index or a sync, each with its progress and, where
//! stopping does something, a way to stop it. Nothing is drawn while idle.
//!
//! A scan or rebuild started here reports through a `ProgressReporter`; a
//! scan the folder watcher started and every sync, wherever it began, are
//! read from the engine's `Tasks` and `Sync` slices, and the covers being
//! fetched ahead from its `Covers` slice.

use std::sync::Arc;

use futures_channel::mpsc;
use futures_util::StreamExt;
use koan_ffi::{ProgressReporter, SyncPhase};
use koan_kit::app::*;

use crate::components::{NavNote, ProgressRow};
use crate::engine::engine;
use crate::mirror::Mirror;
use crate::platform::actions::{ForceRescan, RebuildIndex, Rescan, Sync};
use crate::theme::Tone;

pub struct ActivityHandle(pub Entity<Activity>);

impl Global for ActivityHandle {}

/// A scan this app started, as its reporter last said.
struct Scan {
    force: bool,
    total: u64,
    done: u64,
    detail: SharedString,
}

enum Report {
    Started(u64),
    Advanced(u64, String),
}

struct Reporter(mpsc::UnboundedSender<Report>);

impl ProgressReporter for Reporter {
    fn started(&self, total: u64) {
        self.0.unbounded_send(Report::Started(total)).ok();
    }

    fn advanced(&self, done: u64, detail: String) {
        self.0.unbounded_send(Report::Advanced(done, detail)).ok();
    }
}

pub struct Activity {
    scan: Option<Scan>,
    rebuilding: bool,
    /// A sync asked for here and not yet answered: shown before the engine's
    /// own report of it arrives.
    syncing: bool,
    /// The last task's failure, until another starts.
    problem: Option<SharedString>,
    _watch: [Subscription; 3],
}

/// The library menu's actions, and the activity rows the sidebar shows.
pub fn init(cx: &mut App) {
    let activity = cx.new(|cx: &mut Context<Activity>| {
        let mirror = cx.global::<Mirror>();
        let (tasks, sync, covers) = (
            mirror.tasks.clone(),
            mirror.sync.clone(),
            mirror.covers.clone(),
        );
        Activity {
            scan: None,
            rebuilding: false,
            syncing: false,
            problem: None,
            _watch: [
                cx.observe(&tasks, |_, _, cx| cx.notify()),
                cx.observe(&sync, |_, _, cx| cx.notify()),
                cx.observe(&covers, |_, _, cx| cx.notify()),
            ],
        }
    });
    cx.set_global(ActivityHandle(activity.clone()));
    let (a, b, c, d) = (
        activity.clone(),
        activity.clone(),
        activity.clone(),
        activity,
    );
    cx.on_action(move |_: &Rescan, cx| a.update(cx, |a, cx| a.scan(false, cx)));
    cx.on_action(move |_: &ForceRescan, cx| b.update(cx, |a, cx| a.scan(true, cx)));
    cx.on_action(move |_: &Sync, cx| c.update(cx, |a, cx| a.sync(cx)));
    // Asked first, as Settings asks: what goes with the index is said. A
    // global action runs while GPUI holds the window, so the question waits
    // until it is handed back.
    cx.on_action(move |_: &RebuildIndex, cx| {
        let d = d.clone();
        cx.defer(move |cx| {
            let Some(window) = cx.active_window() else {
                return;
            };
            window
                .update(cx, |_, window, cx| {
                    crate::components::sheet::confirm(
                        crate::components::sheet::Confirm {
                            title: "rebuild the library index".into(),
                            verb: "rebuild".into(),
                            destructive: true,
                            lines: vec![
                                "The next scan re-reads every file and the next sync walks the whole server, and each track takes what its tags say now.".into(),
                                "Play history, playlists, favourites and lyrics are kept. A track nothing finds again goes. Your music files are not touched.".into(),
                            ],
                            note: None,
                        },
                        window,
                        cx,
                        move |cx| d.update(cx, |a, cx| a.rebuild(cx)),
                    )
                })
                .ok();
        })
    });
}

impl Activity {
    fn busy(&self, cx: &App) -> bool {
        self.scan.is_some() || self.rebuilding || cx.global::<Mirror>().tasks.read(cx).0.scanning
    }

    fn scan(&mut self, force: bool, cx: &mut Context<Self>) {
        // One library task at a time, as the engine's lane runs them.
        if self.busy(cx) {
            return;
        }
        self.problem = None;
        self.scan = Some(Scan {
            force,
            total: 0,
            done: 0,
            detail: SharedString::default(),
        });
        cx.notify();
        let (sender, mut reports) = mpsc::unbounded();
        let reporter: Arc<dyn ProgressReporter> = Arc::new(Reporter(sender));
        let scanning = engine(cx).scan_reporting(force, Some(reporter));
        // Reports arrive per file; each batch that is ready is one redraw.
        cx.spawn(async move |this, cx| {
            while let Some(first) = reports.next().await {
                let mut batch = vec![first];
                while let Ok(next) = reports.try_recv() {
                    batch.push(next);
                }
                let alive = this.update(cx, |this, cx| {
                    let Some(scan) = this.scan.as_mut() else {
                        return;
                    };
                    for report in batch {
                        match report {
                            Report::Started(total) => scan.total = total,
                            Report::Advanced(done, detail) => {
                                scan.done = done;
                                scan.detail = detail.into();
                            }
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = scanning.await;
            this.update(cx, |this, cx| {
                this.scan = None;
                if let Err(e) = result {
                    log::warn!("scan: {e}");
                    this.problem = Some(format!("scan failed: {e}").into());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        if self.syncing || cx.global::<Mirror>().tasks.read(cx).0.syncing {
            return;
        }
        self.problem = None;
        self.syncing = true;
        cx.notify();
        let syncing = engine(cx).sync_remote();
        cx.spawn(async move |this, cx| {
            let result = syncing.await;
            this.update(cx, |this, cx| {
                this.syncing = false;
                if let Err(e) = result {
                    log::warn!("sync: {e}");
                    this.problem = Some(format!("sync failed: {e}").into());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        // Nor under a sync, whose pages it would delete as they land.
        if self.busy(cx) || self.syncing || cx.global::<Mirror>().tasks.read(cx).0.syncing {
            return;
        }
        self.problem = None;
        self.rebuilding = true;
        cx.notify();
        let rebuilding = engine(cx).rebuild_index();
        cx.spawn(async move |this, cx| {
            let result = rebuilding.await;
            this.update(cx, |this, cx| {
                this.rebuilding = false;
                if let Err(e) = result {
                    log::warn!("rebuild: {e}");
                    this.problem = Some(format!("rebuild failed: {e}").into());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The rows to draw, in the order they started being likely to.
    fn rows(&self, cx: &App) -> Vec<Row> {
        let mirror = cx.global::<Mirror>();
        let tasks = mirror.tasks.read(cx).0;
        let mut rows = Vec::new();
        match &self.scan {
            Some(scan) => rows.push(Row {
                id: "activity-scan",
                label: if scan.force {
                    "rescanning all"
                } else {
                    "rescanning"
                }
                .into(),
                done: (scan.total > 0 || scan.done > 0).then_some(scan.done),
                total: Some(scan.total).filter(|t| *t > 0),
                unit: Some("files"),
                detail: Some(scan.detail.clone()).filter(|d| !d.is_empty()),
                stop: Some(stop_library_task),
            }),
            // The folder watcher's, which this app did not start.
            None if tasks.scanning => rows.push(Row {
                id: "activity-scan",
                label: "scanning folders".into(),
                done: None,
                total: None,
                unit: None,
                detail: None,
                stop: Some(stop_library_task),
            }),
            None => {}
        }
        if self.rebuilding {
            rows.push(Row {
                id: "activity-rebuild",
                label: "rebuilding the index".into(),
                done: None,
                total: None,
                unit: None,
                detail: None,
                stop: None,
            });
        }
        if self.syncing || tasks.syncing {
            let progress = mirror.sync.read(cx).0;
            let (label, unit) = match progress.as_ref().map(|p| p.phase) {
                Some(SyncPhase::Albums) => ("syncing albums", Some("albums")),
                Some(SyncPhase::Tracks) => ("syncing tracks", Some("tracks")),
                Some(SyncPhase::Artists) => ("syncing artists", Some("artists")),
                Some(SyncPhase::Finishing) => ("finishing the sync", None),
                None => ("syncing", None),
            };
            rows.push(Row {
                id: "activity-sync",
                label: label.into(),
                done: progress.as_ref().filter(|_| unit.is_some()).map(|p| p.done),
                total: progress.as_ref().and_then(|p| p.total).filter(|t| *t > 0),
                unit,
                detail: None,
                stop: None,
            });
        }
        if let Some(covers) = mirror.covers.read(cx).0 {
            rows.push(Row {
                id: "activity-covers",
                label: "fetching covers".into(),
                done: Some(covers.done),
                total: Some(covers.total).filter(|t| *t > 0),
                unit: Some("records"),
                detail: None,
                stop: Some(stop_cover_preload),
            });
        }
        rows
    }
}

struct Row {
    id: &'static str,
    label: SharedString,
    done: Option<u64>,
    total: Option<u64>,
    unit: Option<&'static str>,
    detail: Option<SharedString>,
    /// What its stop button does; no button without one.
    stop: Option<fn(&App)>,
}

fn stop_library_task(cx: &App) {
    engine(cx).cancel_library_task();
}

fn stop_cover_preload(cx: &App) {
    engine(cx).stop_cover_preload();
}

impl Row {
    /// "12,400 of 49,700 files", or the count alone without a total.
    fn counts(&self) -> Option<String> {
        let done = self.done?;
        let unit = self.unit.map(|u| format!(" {u}")).unwrap_or_default();
        Some(match self.total {
            Some(total) => format!("{} of {}{unit}", grouped(done), grouped(total)),
            None => format!("{}{unit}", grouped(done)),
        })
    }

    fn fraction(&self) -> Option<f32> {
        let total = self.total.filter(|t| *t > 0)?;
        Some((self.done.unwrap_or(0) as f32 / total as f32).clamp(0.0, 1.0))
    }
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Render for Activity {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        if rows.is_empty() && self.problem.is_none() {
            return div().into_any_element();
        }
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .children(rows.iter().map(|row| {
                let mut progress =
                    ProgressRow::new(row.id, row.label.clone()).fraction(row.fraction());
                if let Some(counts) = row.counts() {
                    progress = progress.counts(counts);
                }
                if let Some(detail) = row.detail.clone() {
                    progress = progress.detail(detail);
                }
                if let Some(stop) = row.stop {
                    progress = progress.on_stop(move |_, _, cx| stop(cx));
                }
                progress
            }))
            .children(
                self.problem
                    .clone()
                    .map(|problem| NavNote::new(problem, Tone::Bad)),
            )
            .into_any_element()
    }
}

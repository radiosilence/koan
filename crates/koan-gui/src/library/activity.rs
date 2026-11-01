//! What the library is busy with, as rows at the foot of the sidebar: a scan,
//! a rebuild of the index or a sync, each with its progress and, where
//! stopping does something, a way to stop it. Nothing is drawn while idle.
//!
//! A scan or rebuild started here reports through a `ProgressReporter`; a
//! scan the folder watcher started and every sync, wherever it began, are
//! read from the engine's `Tasks` and `Sync` slices.

use std::sync::Arc;
use std::time::Duration;

use futures_channel::mpsc;
use futures_util::StreamExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{ProgressReporter, SyncPhase};

use crate::engine::engine;
use crate::mirror::Mirror;
use crate::paint::Paint;
use crate::platform::actions::{ForceRescan, RebuildIndex, Rescan, Sync};
use crate::theme::{Icon, KoanIcon, KoanStyled, KoanTheme, Space, TypeRole, tokens::Motion};

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
    _watch: [Subscription; 2],
}

/// The library menu's actions, and the activity rows the sidebar shows.
pub fn init(cx: &mut App) {
    let activity = cx.new(|cx: &mut Context<Activity>| {
        let mirror = cx.global::<Mirror>();
        let (tasks, sync) = (mirror.tasks.clone(), mirror.sync.clone());
        Activity {
            scan: None,
            rebuilding: false,
            syncing: false,
            problem: None,
            _watch: [
                cx.observe(&tasks, |_, _, cx| cx.notify()),
                cx.observe(&sync, |_, _, cx| cx.notify()),
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
    cx.on_action(move |_: &RebuildIndex, cx| d.update(cx, |a, cx| a.rebuild(cx)));
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
                while let Ok(Some(next)) = reports.try_next() {
                    batch.push(next);
                }
                let alive = this.update(cx, |this, cx| {
                    let Some(scan) = this.scan.as_mut() else { return };
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
        if self.busy(cx) {
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
                label: if scan.force { "rescanning all" } else { "rescanning" }.into(),
                done: Some(scan.done).filter(|_| scan.total > 0 || scan.done > 0),
                total: Some(scan.total).filter(|t| *t > 0),
                unit: Some("files"),
                detail: Some(scan.detail.clone()).filter(|d| !d.is_empty()),
                cancellable: true,
            }),
            // The folder watcher's, which this app did not start.
            None if tasks.scanning => rows.push(Row {
                id: "activity-scan",
                label: "scanning folders".into(),
                done: None,
                total: None,
                unit: None,
                detail: None,
                cancellable: true,
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
                cancellable: false,
            });
        }
        if self.syncing || tasks.syncing {
            let progress = mirror.sync.read(cx).0.clone();
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
                cancellable: false,
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
    cancellable: bool,
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

/// The bar: the done fraction where the task can say, a sliding segment
/// where it cannot, the same height either way so a total arriving partway
/// does not move the row. Two quads; a sliding segment only while motion is
/// allowed.
fn bar(row: &Row, p: &Paint, cx: &App) -> AnyElement {
    let track = div().w_full().h(px(2.)).bg(p.rule).relative().overflow_hidden();
    match row.fraction() {
        Some(fraction) => track
            .child(div().h_full().w(relative(fraction)).bg(p.accent))
            .into_any_element(),
        None if KoanTheme::motion_allowed(cx) => track
            .child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(relative(0.3))
                    .bg(p.accent)
                    .with_animation(
                        (row.id, 0u64),
                        Animation::new(Duration::from_millis(1200)).repeat(),
                        |segment, t| segment.left(relative(-0.3 + 1.3 * Motion::ease_in_out(t))),
                    ),
            )
            .into_any_element(),
        None => track
            .child(div().h_full().w_full().bg(p.accent.opacity(0.4)))
            .into_any_element(),
    }
}

impl Render for Activity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let rows = self.rows(cx);
        if rows.is_empty() && self.problem.is_none() {
            return div().into_any_element();
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(p.space(Space::S))
            .px(p.space(Space::L))
            .pb(p.space(Space::Xs))
            .children(rows.iter().map(|row| {
                let cancel = row.cancellable.then(|| {
                    div()
                        .id((row.id, 1u64))
                        .flex_none()
                        .cursor_pointer()
                        .text_color(p.muted.opacity(0.6))
                        .hover(|s| s.text_color(p.muted))
                        .child(KoanIcon::new(Icon::Deselect).role(TypeRole::Fine).color(p.muted))
                        .on_click(|_, _, cx| engine(cx).cancel_library_task())
                });
                div()
                    .id(row.id)
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .gap(p.space(Space::S))
                            .text_role(TypeRole::Fine, &p.look)
                            .child(div().flex_1().text_color(p.muted).child(row.label.clone()))
                            .children(
                                row.counts()
                                    .map(|c| div().text_color(p.muted.opacity(0.7)).child(c)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(p.space(Space::S))
                            .child(div().flex_1().child(bar(row, &p, cx)))
                            .children(cancel),
                    )
                    .children(row.detail.clone().map(|detail| {
                        div()
                            .text_role(TypeRole::Fine, &p.look)
                            .text_color(p.muted.opacity(0.7))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(detail)
                    }))
            }))
            .children(self.problem.clone().map(|problem| {
                div()
                    .text_role(TypeRole::Fine, &p.look)
                    .text_color(p.bad)
                    .child(problem)
            }))
            .into_any_element()
    }
}

//! The downloads page: what koan is fetching from the server, and what it
//! just finished or failed to fetch, until those are cleared. The first place
//! to look when something does not play: a transfer that has stopped moving
//! says so, its rate falling to nothing while its bar stays put, and a
//! failure keeps its reason.
//!
//! The list follows the Transfers slice, which moves a handful of times in a
//! transfer's life. A running row's bar reads the bytes each frame it is
//! drawn (`transfers::progress`) and asks for the next one; nothing running,
//! or the page not on stage, and no frame is asked for. The rate beside it is
//! the Figures slice's, a few readings a second.
//!
//! Every write says what it changes first: cancelling fails the queue
//! entries waiting on the track, removing a file deletes it, and clearing
//! forgets the settled rows.

use koan_ffi::{Transfer, TransferFigure, TransferState};
use koan_kit::app::*;

use crate::components::sheet as ask;
use crate::components::{
    Button, ButtonKind, EmptyState, Page, PageHeader, ProgressBar, RowKind, ScrollArea, TableRow,
};
use crate::engine::{command, engine};
use crate::mirror::{Mirror, Slice};
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::theme::{Icon, KoanStyled, TypeRole};
use crate::time::bytes;

/// The status column's width, in points: "12.3 MB/s · 45.6 MB of 78.9 MB".
const STATUS: f32 = 200.0;
/// The bar's, beside it.
const BAR: f32 = 80.0;
/// A row's verb.
const VERB: f32 = 64.0;

pub struct DownloadsPage {
    nav: Entity<Navigator>,
    transfers: Entity<Slice<Vec<Transfer>>>,
    figures: Entity<Slice<Vec<TransferFigure>>>,
    cache: Entity<Slice<u64>>,
    /// The cache's limit in bytes, `None` for none; read as the page opens.
    limit: Option<u64>,
    _limit: Task<()>,
    _subscriptions: [Subscription; 3],
}

impl DownloadsPage {
    pub fn new(nav: &Entity<Navigator>, cx: &mut Context<Self>) -> Self {
        let mirror = cx.global::<Mirror>().clone();
        let read = engine(cx).cache_limit_bytes();
        Self {
            nav: nav.clone(),
            _subscriptions: [
                cx.observe(&mirror.transfers, |_, _, cx| cx.notify()),
                // The rates; a running row's bar takes frames of its own.
                cx.observe(&mirror.figures, |_, _, cx| cx.notify()),
                cx.observe(&mirror.cache, |_, _, cx| cx.notify()),
            ],
            transfers: mirror.transfers,
            figures: mirror.figures,
            cache: mirror.cache,
            limit: None,
            _limit: cx.spawn(async move |this, cx| {
                let limit = read.await;
                this.update(cx, |this, cx| {
                    this.limit = limit;
                    cx.notify();
                })
                .ok();
            }),
        }
    }

    fn header(&self, settled: usize, cx: &mut Context<Self>) -> PageHeader {
        let used = self.cache.read(cx).0;
        let detail = match self.limit {
            Some(limit) => format!("{} cached of {}", bytes(used), bytes(limit)),
            None => format!("{} cached", bytes(used)),
        };
        let header = PageHeader::new("downloads").detail(detail);
        if settled == 0 {
            return header;
        }
        header.action(
            Button::new("clear-finished", ButtonKind::Compact)
                .icon(Icon::Clear)
                .label("clear finished")
                .on_click(move |_, window, cx| ask_clear_settled(settled, window, cx)),
        )
    }

    fn row(&self, t: &Transfer, p: &Paint, window: &mut Window, cx: &App) -> AnyElement {
        let id = t.track_id;
        let figure = self
            .figures
            .read(cx)
            .0
            .iter()
            .find(|f| f.track_id == id)
            .cloned();
        let (status, warns) = match t.state {
            TransferState::Queued => ("queued".to_owned(), false),
            TransferState::Running => (
                figure.as_ref().map(rate_and_size).unwrap_or_default(),
                false,
            ),
            TransferState::Done => (
                figure
                    .as_ref()
                    .filter(|f| f.total_bytes > 0)
                    .map_or_else(|| "done".to_owned(), |f| bytes(f.total_bytes)),
                false,
            ),
            TransferState::Failed => ("failed".to_owned(), true),
        };
        let line = match (&t.failure_reason, t.album.is_empty()) {
            (Some(reason), _) => reason.clone(),
            (None, true) => t.artist.clone(),
            (None, false) => format!("{} — {}", t.artist, t.album),
        };
        let bar = (t.state == TransferState::Running).then(|| {
            // The bytes land between the list's own redraws: read again on
            // the next frame, for as long as this row is drawn.
            window.request_animation_frame();
            ProgressBar::new(("bar", id as u64), crate::transfers::progress(id, cx))
        });
        let row = TableRow::new(("download", id as u64), RowKind::Text)
            .title(
                div()
                    .text_role(TypeRole::Body, &p.look)
                    .text_color(p.ink)
                    .truncate()
                    .child(t.title.clone()),
            )
            .line(
                div()
                    .text_role(TypeRole::Meta, &p.look)
                    .text_color(if t.failure_reason.is_some() {
                        p.bad
                    } else {
                        p.muted
                    })
                    .truncate()
                    .child(line),
            )
            .figure(BAR, div().w_full().children(bar))
            .figure(
                STATUS,
                div()
                    .flex()
                    .justify_end()
                    .text_role(TypeRole::Fine, &p.look)
                    .text_color(if warns { p.bad } else { p.muted })
                    .child(status),
            )
            .figure(VERB, verb(t));
        let album = t.album_id;
        let nav = self.nav.clone();
        div()
            .id(("download-row", id as u64))
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                // The record it came off, which is where to find it.
                if let Some(album) = album {
                    nav.update(cx, |nav, cx| {
                        if nav.go(Route::Album(album)) {
                            cx.notify();
                        }
                    });
                }
            })
            .child(row)
            .into_any_element()
    }
}

impl Render for DownloadsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let transfers = self.transfers.read(cx).0.clone();
        let settled = transfers
            .iter()
            .filter(|t| matches!(t.state, TransferState::Done | TransferState::Failed))
            .count();
        let header = self.header(settled, cx);
        let body = if transfers.is_empty() {
            EmptyState::new(Icon::Downloads, "nothing downloading")
                .detail("Tracks fetched from your server appear here while they arrive.")
                .into_any_element()
        } else {
            let rows: Vec<AnyElement> = transfers
                .iter()
                .map(|t| self.row(t, &p, window, cx))
                .collect();
            ScrollArea::new("downloads", div().flex().flex_col().children(rows)).into_any_element()
        };
        Page::new(header).body(body)
    }
}

/// What a running transfer's status says: its rate, or that it has stalled,
/// and how much of how much has arrived.
fn rate_and_size(f: &TransferFigure) -> String {
    let rate = if f.bytes_per_second > 0 {
        format!("{}/s", bytes(f.bytes_per_second))
    } else {
        "stalled".to_owned()
    };
    match (f.bytes_written, f.total_bytes) {
        (_, 0) if f.bytes_written == 0 => rate,
        (written, 0) => format!("{rate} · {}", bytes(written)),
        (written, total) => format!("{rate} · {} of {}", bytes(written), bytes(total)),
    }
}

/// A row's one verb: retry a failure, cancel what is still to come, remove
/// a file that arrived.
fn verb(t: &Transfer) -> AnyElement {
    let id = t.track_id;
    let title = t.title.clone();
    let (label, icon) = match t.state {
        TransferState::Failed => ("retry", Icon::Rescan),
        TransferState::Queued | TransferState::Running => ("cancel", Icon::Close),
        TransferState::Done => ("remove", Icon::Clear),
    };
    let state = t.state;
    Button::new(("verb", id as u64), ButtonKind::Compact)
        .icon(icon)
        .label(label)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            match state {
                TransferState::Failed => {
                    command(cx, "retry download", move |e| e.retry_downloads(vec![id]))
                }
                TransferState::Queued | TransferState::Running => {
                    ask_cancel(id, &title, window, cx)
                }
                TransferState::Done => ask_remove(id, &title, window, cx),
            }
        })
        .into_any_element()
}

fn ask_cancel(id: i64, title: &str, window: &mut Window, cx: &mut App) {
    ask::confirm(
        ask::Confirm {
            title: "cancel download".into(),
            verb: "cancel download".into(),
            destructive: false,
            lines: vec![format!("“{title}” stops downloading.").into()],
            note: Some(
                "It stays here as cancelled, and the queue skips it, until it is retried.".into(),
            ),
        },
        window,
        cx,
        move |cx| command(cx, "cancel download", move |e| e.cancel_downloads(vec![id])),
    );
}

fn ask_remove(id: i64, title: &str, window: &mut Window, cx: &mut App) {
    ask::confirm(
        ask::Confirm {
            title: "remove downloaded file".into(),
            verb: "remove".into(),
            destructive: true,
            lines: vec![format!("The copy of “{title}” on this device is deleted.").into()],
            note: Some("It stays on the server and downloads again when played.".into()),
        },
        window,
        cx,
        move |cx| command(cx, "remove download", move |e| e.clear_downloads(vec![id])),
    );
}

fn ask_clear_settled(settled: usize, window: &mut Window, cx: &mut App) {
    ask::confirm(
        ask::Confirm {
            title: "clear finished".into(),
            verb: "clear".into(),
            destructive: false,
            lines: vec![
                format!(
                    "{} leave this list.",
                    crate::library::chrome::count(
                        settled as u64,
                        "finished or failed download",
                        "finished and failed downloads"
                    )
                )
                .into(),
            ],
            note: Some("Downloaded files stay on this device; downloads running carry on.".into()),
        },
        window,
        cx,
        |cx| engine(cx).clear_settled_downloads(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn figure(written: u64, total: u64, rate: u64) -> TransferFigure {
        TransferFigure {
            track_id: 1,
            progress: None,
            bytes_written: written,
            total_bytes: total,
            bytes_per_second: rate,
        }
    }

    #[test]
    fn a_running_transfer_says_its_rate_and_how_far() {
        assert_eq!(
            rate_and_size(&figure(3_000_000, 9_000_000, 1_500_000)),
            "1.5 MB/s · 3 MB of 9 MB"
        );
        assert_eq!(rate_and_size(&figure(3_000_000, 0, 0)), "stalled · 3 MB");
        assert_eq!(rate_and_size(&figure(0, 0, 0)), "stalled");
    }
}

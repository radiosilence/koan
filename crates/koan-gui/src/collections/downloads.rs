//! What is arriving from the server: each transfer with its bar, its rate
//! and its size, and the ones that settled until they are cleared.
//!
//! The list follows the engine's transfers, which change a handful of times
//! in a transfer's life. The bars follow the bytes: while a transfer runs and
//! this page is drawn, each frame reads the byte counts straight from the
//! engine (`transfer_readings`, a lock held for one pass over a few dozen
//! entries) and asks for the next frame. Nothing running, or the page not
//! drawn, and no frame is asked for: an idle page costs nothing.

use std::collections::HashMap;
use std::ops::Range;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Transfer, TransferFigure, TransferState};

use super::{Confirm, Picks, confirm, count};
use crate::components::{KoanContextMenu, MenuEntry};
use crate::library::covers::{Art, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use crate::library::nav::Page;
use crate::library::tokens::{Paint, Type, Typeset};
use crate::library::{Lib, chrome, rows, tile};
use crate::mirror::{Mirror, Slice};
use crate::theme::{Icon, Space};

const BAR: f32 = 3.0;

/// "1.2 MB", in decimal units as the Mac shows them.
pub fn bytes(n: u64) -> String {
    let n = n as f64;
    match n {
        n if n >= 1e9 => format!("{:.1} GB", n / 1e9),
        n if n >= 1e6 => format!("{:.1} MB", n / 1e6),
        n if n >= 1e3 => format!("{:.0} KB", n / 1e3),
        n => format!("{n:.0} bytes"),
    }
}

/// What a running transfer's status line says: its rate, or that it has
/// stalled, and how much of how much has arrived.
pub fn rate_and_size(f: &TransferFigure) -> String {
    let mut parts = vec![if f.bytes_per_second > 0 {
        format!("{}/s", bytes(f.bytes_per_second))
    } else {
        "stalled".to_owned()
    }];
    if f.total_bytes > 0 {
        parts.push(format!(
            "{} of {}",
            bytes(f.bytes_written),
            bytes(f.total_bytes)
        ));
    } else if f.bytes_written > 0 {
        parts.push(bytes(f.bytes_written));
    }
    parts.join(" · ")
}

pub struct DownloadsPage {
    lib: Lib,
    transfers: Entity<Slice<Vec<Transfer>>>,
    figures: Entity<Slice<Vec<TransferFigure>>>,
    /// This frame's byte counts, read while something runs.
    readings: HashMap<i64, TransferFigure>,
    scroll: UniformListScrollHandle,
    picks: Picks,
    menu_track: Option<i64>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl DownloadsPage {
    pub fn new(lib: Lib, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.entity_id();
        let mirror = cx.global::<Mirror>().clone();
        let subscriptions = vec![
            crate::theme::observe_accent(cx),
            cx.observe(&mirror.transfers, |this, transfers, cx| {
                let order: Vec<i64> = transfers.read(cx).0.iter().map(|t| t.track_id).collect();
                this.picks.retain(&order);
                cx.notify();
            }),
            // The rate, a few times a second; the bars take frames of their own.
            cx.observe(&mirror.figures, |_, _, cx| cx.notify()),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.on_release(move |this, cx| {
                this.lib.covers.update(cx, |c, _| c.release(owner));
            }),
        ];
        Self {
            lib,
            transfers: mirror.transfers,
            figures: mirror.figures,
            readings: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            picks: Picks::default(),
            menu_track: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn row_height(p: &Paint) -> Pixels {
        rows::art_height(p) + p.look.line_height(Type::Fine) + px(BAR) + p.space(Space::Xs)
    }

    fn show_in_library(&self, t: &Transfer, cx: &mut App) {
        if let Some(album) = t.album_id {
            self.lib
                .nav
                .update(cx, |nav, cx| nav.open(Page::Album(album), cx));
        }
    }

    fn ask_remove_file(&self, t: &Transfer, window: &mut Window, cx: &mut App) {
        let (engine, id) = (self.lib.engine.clone(), t.track_id);
        confirm(
            Confirm {
                title: "remove downloaded file".into(),
                verb: "remove".into(),
                destructive: true,
                lines: vec![format!("The copy of “{}” on this device is deleted.", t.title).into()],
                note: Some("It stays on the server and downloads again when played.".into()),
            },
            window,
            cx,
            move |cx| {
                let engine = engine.clone();
                cx.spawn(async move |_| {
                    if let Err(e) = engine.clear_downloads(vec![id]).await {
                        log::warn!("remove download {id}: {e}");
                    }
                })
                .detach();
            },
        );
    }

    fn aim(&mut self, visible: Range<usize>, scale: f32, cx: &mut Context<Self>) {
        let ranked: Vec<(Key, Priority)> = self.transfers.read(cx).0[visible]
            .iter()
            .filter_map(|t| {
                Some((
                    Key::new(Art::Album(t.album_id?), px(rows::SLEEVE), scale),
                    Priority::Visible,
                ))
            })
            .collect();
        let owner = cx.entity_id();
        self.lib.covers.update(cx, |c, _| c.request(owner, ranked));
    }

    fn row(&mut self, p: &Paint, ix: usize, scale: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(t) = self.transfers.read(cx).0.get(ix).cloned() else {
            return div().into_any_element();
        };
        let figure = self.readings.get(&t.track_id).cloned().or_else(|| {
            self.figures
                .read(cx)
                .0
                .iter()
                .find(|f| f.track_id == t.track_id)
                .cloned()
        });
        let sleeve = t.album_id.map(|album| {
            let key = Key::new(Art::Album(album), px(rows::SLEEVE), scale);
            self.lib.covers.update(cx, |c, _| c.sleeve(key))
        });
        let fraction = match t.state {
            TransferState::Done => 1.0,
            _ => figure.as_ref().and_then(|f| f.progress).unwrap_or(0.0) as f32,
        };
        let (status, tone) = match t.state {
            TransferState::Failed => (
                t.failure_reason
                    .clone()
                    .unwrap_or_else(|| "could not be fetched".into()),
                p.bad,
            ),
            TransferState::Queued => ("waiting".into(), p.muted),
            TransferState::Done => ("downloaded".into(), p.muted),
            TransferState::Running => (
                figure.as_ref().map(rate_and_size).unwrap_or_default(),
                p.muted,
            ),
        };
        let mark = match t.state {
            TransferState::Done => "done".to_owned(),
            TransferState::Failed => "failed".to_owned(),
            TransferState::Queued => "queued".to_owned(),
            TransferState::Running => figure
                .as_ref()
                .and_then(|f| f.progress)
                .map(|f| format!("{}%", (f * 100.0) as u32))
                .unwrap_or_default(),
        };
        let running = t.state == TransferState::Running;
        let credit = if t.album.is_empty() {
            t.artist.clone()
        } else {
            format!("{} · {}", t.artist, t.album)
        };
        let id = t.track_id;
        let selected = self.picks.contains(id);
        let mut art = div().size(px(rows::SLEEVE)).flex_none().bg(p.surface);
        match sleeve {
            Some(Sleeve::Ready(image)) => {
                art = art.child(img(ImageSource::Render(image)).size_full())
            }
            Some(Sleeve::Colour(colour)) => art = art.bg(colour),
            _ => {}
        }
        div()
            .id(("transfer", id as u64))
            .h(Self::row_height(p))
            .flex()
            .items_center()
            .gap(p.space(Space::M))
            .px(px(tile::MARGIN))
            .when(selected, |d| d.bg(p.surface))
            .when(!selected, |d| d.hover(|s| s.bg(p.row_hover())))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.menu_track = Some(id)),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                let order: Vec<i64> = this
                    .transfers
                    .read(cx)
                    .0
                    .iter()
                    .map(|t| t.track_id)
                    .collect();
                if this.picks.click(id, event, &order, cx)
                    && let Some(t) = this.transfers.read(cx).0.iter().find(|t| t.track_id == id)
                {
                    let t = t.clone();
                    this.show_in_library(&t, cx);
                }
                cx.notify();
            }))
            .child(art)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap(p.space(Space::M))
                            .child(
                                div()
                                    .truncate()
                                    .typeset(p, Type::Body, p.ink)
                                    .child(t.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .typeset(p, Type::Fine, p.muted)
                                    .child(mark),
                            ),
                    )
                    // The bar: the arrived part in the accent while it runs.
                    .child(
                        div().h(px(BAR)).w_full().bg(p.rule).child(
                            div()
                                .h_full()
                                .w(relative(fraction.clamp(0.0, 1.0)))
                                .bg(if running { p.accent } else { p.ink }),
                        ),
                    )
                    .child(
                        div()
                            .truncate()
                            .typeset(p, Type::Meta, p.muted)
                            .child(credit),
                    )
                    .child(div().truncate().typeset(p, Type::Fine, tone).child(status)),
            )
            .into_any_element()
    }
}

impl Render for DownloadsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let transfers = self.transfers.read(cx).0.clone();
        // Display rate while bytes are arriving and the page is drawn.
        self.readings.clear();
        if transfers.iter().any(|t| t.state == TransferState::Running) {
            self.readings = self
                .lib
                .engine
                .transfer_readings()
                .into_iter()
                .map(|f| (f.track_id, f))
                .collect();
            window.request_animation_frame();
        }
        let scale = window.scale_factor();
        let first = self.scroll.logical_scroll_top_index();
        let shown = (first..(first + 30).min(transfers.len())).clone();
        self.aim(shown.start.min(transfers.len())..shown.end, scale, cx);
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        let running = transfers.iter().filter(|t| !t.state.is_settled()).count();
        let settled = transfers.len() - running;
        let mut detail = Vec::new();
        if running > 0 {
            detail.push(format!("{running} arriving"));
        }
        if settled > 0 {
            detail.push(format!("{settled} finished"));
        }
        let engine = self.lib.engine.clone();
        let header = chrome::page_header(
            p,
            "downloads",
            if detail.is_empty() {
                count(0, "transfer", "transfers")
            } else {
                detail.join(" · ")
            },
            div().when(settled > 0, |d| {
                // Forgets the finished rows; nothing on disk changes.
                d.child(
                    chrome::text_button("clear-finished", "clear finished")
                        .on_click(move |_, _, _| engine.clear_settled_downloads()),
                )
            }),
        );
        let body = if transfers.is_empty() {
            chrome::empty_state(
                Icon::Downloads,
                "nothing downloading",
                "tracks fetched from your server appear here while they arrive.",
            )
            .into_any_element()
        } else {
            uniform_list(
                "downloads",
                transfers.len(),
                cx.processor(move |this, range: Range<usize>, window, cx| {
                    let p = Paint::of(window, cx);
                    range
                        .map(|ix| this.row(&p, ix, scale, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };
        let weak = cx.weak_entity();
        div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .typeset(p, Type::Body, p.ink)
            .child(header)
            .child(
                div()
                    .id("downloads-list")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .pt(px(10.0))
                    .capture_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, _, _| {
                        if e.button == MouseButton::Right {
                            this.menu_track = None;
                        }
                    }))
                    .child(body)
                    .child(gpui_kit::base::Scrollbar::vertical(&self.scroll))
                    .koan_context_menu("downloads-menu", window, cx, move |m, _, cx| {
                        let Some(view) = weak.upgrade() else { return m };
                        let page = view.read(cx);
                        let Some(t) = page.menu_track.and_then(|id| {
                            page.transfers
                                .read(cx)
                                .0
                                .iter()
                                .find(|t| t.track_id == id)
                                .cloned()
                        }) else {
                            return m;
                        };
                        let mut m = m;
                        if t.album_id.is_some() {
                            let (view, t) = (view.clone(), t.clone());
                            m = m.item(MenuEntry::new("show in library").on_click(
                                move |_, _, cx| {
                                    view.update(cx, |this, cx| this.show_in_library(&t, cx))
                                },
                            ));
                        }
                        if t.state == TransferState::Done {
                            let view = view.clone();
                            m = m.item(
                                MenuEntry::new("remove downloaded file")
                                    .destructive()
                                    .on_click(move |_, window, cx| {
                                        view.update(cx, |this, cx| {
                                            this.ask_remove_file(&t, window, cx)
                                        })
                                    }),
                            );
                        }
                        m
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn a_running_transfer_says_its_rate_and_size() {
        let f = TransferFigure {
            track_id: 1,
            progress: Some(0.5),
            bytes_written: 15_000_000,
            total_bytes: 30_000_000,
            bytes_per_second: 2_400_000,
        };
        assert_eq!(rate_and_size(&f), "2.4 MB/s · 15.0 MB of 30.0 MB");
        let stalled = TransferFigure {
            bytes_per_second: 0,
            total_bytes: 0,
            ..f
        };
        assert_eq!(rate_and_size(&stalled), "stalled · 15.0 MB");
    }
}

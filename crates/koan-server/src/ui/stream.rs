//! The live stream: one route, drawing whichever page the address the page
//! sends (`$url`) names. Every page with parts that change on their own has
//! a [`Live`] of its own under this module, named as the page is; the rest
//! are [`Quiet`]. A stream never tracks navigation: the shell opens it afresh
//! when `$url` changes, so it always draws the page the address names.
//!
//! Nothing is polled. A stream waits on what changes its page (the job
//! registry's and the cover cache's signals), paced by
//! `askama_datastar::paced`: at most one redraw every quarter second, and the
//! stream ended after a minute for the page's reopen attributes to open
//! another, which bounds how long a page sits on a connection that died
//! without either end noticing. A region is sent only when it differs from
//! what was last sent (`askama_datastar::Last`).
//!
//! **The seen handover.** A page records what it was drawn up to (`seen`):
//! this process's boot and each topic's count, read before its body was
//! drawn. A stream's first pass draws only the topics that moved since, so
//! nothing the body drew is sent again, and every pass that draws the page's
//! regions sets `seen` beside them, in the same answer: Datastar drops a
//! stream without telling the server when the page is hidden or the network
//! fails, and the stream that replaces it reads `seen` from the page. A
//! `seen` from another boot is drawn whole, since the counts started again at
//! zero. A pass that could not read what it draws withholds `seen` and ends
//! the stream, so the one reopened draws what it missed; a pass that drew
//! nothing of the page's own does not move it.

use std::future::Future;
use std::sync::Arc;

use askama_datastar::{Last, Pace, Signals, paced};
use axum::Extension;
use axum::extract::State;
use axum::response::Response;
use axum::response::sse::Event;
use futures_util::StreamExt as _;
use tokio::sync::watch;

use super::UiState;
use crate::auth::AuthUser;

pub(super) mod library;

/// What a page's live parts follow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Topic {
    /// Scans: started, moving, ended (the job registry).
    Scans,
    /// The cover cache: its warm and sweeps.
    Covers,
}

const TOPICS: usize = 2;

/// This process, among those a page may have been drawn by: a random id,
/// made once when the UI starts.
#[derive(Clone)]
pub(super) struct Boot(Arc<str>);

impl Boot {
    pub(super) fn new() -> Self {
        Self(uuid::Uuid::now_v7().simple().to_string().into())
    }
}

/// What a page has been drawn up to.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(super) struct Seen {
    boot: String,
    counts: [u64; TOPICS],
}

impl Seen {
    /// Where every topic stands now.
    pub(super) fn now(s: &UiState) -> Self {
        Self {
            boot: s.boot.0.to_string(),
            counts: [*s.jobs.subscribe().borrow(), *s.covers.subscribe().borrow()],
        }
    }

    /// As the page carries it.
    pub(super) fn signal(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// The topics that moved between `drawn` and `self`: every one when the
    /// page says nothing of what it drew, or was drawn by another boot.
    fn moved_since(&self, drawn: Option<&Seen>) -> [bool; TOPICS] {
        match drawn {
            Some(d) if d.boot == self.boot => {
                std::array::from_fn(|i| d.counts[i] != self.counts[i])
            }
            _ => [true; TOPICS],
        }
    }
}

/// What changed since the page's last pass.
pub(super) struct Changed {
    topics: [bool; TOPICS],
    /// Where the topics stood when this pass began: what `seen` becomes once
    /// the pass draws.
    seen: Seen,
}

impl Changed {
    pub(super) fn topic(&self, t: Topic) -> bool {
        self.topics[t as usize]
    }
}

/// What the page shows, from the signals Datastar sends with the stream's
/// request.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(super) struct Shown {
    /// The page's address, path and query, as the address bar shows it.
    pub(super) url: String,
    /// What it has been drawn up to; none from a page that does not say,
    /// which is drawn whole.
    pub(super) seen: Option<Seen>,
}

/// What a pass of a page's stream did.
pub(super) enum Pass {
    /// Drew the page's regions as the topics stand: these are the ones that
    /// differ from what was last sent, and `seen` moves on.
    Drew(Vec<Event>),
    /// Drew nothing of the page's own; `seen` stays.
    Nothing,
    /// Could not read what it draws: `seen` stays, and the stream ends for
    /// the next to draw what this one missed.
    Failed,
}

/// A page's live parts.
pub(super) trait Live: Sized + Send + 'static {
    /// Ready to draw the page `shown` describes for `user`.
    fn open(s: &UiState, user: &AuthUser, shown: &Shown) -> Self;
    /// The regions `changed` touches.
    fn draw(&mut self, s: &UiState, changed: &Changed) -> impl Future<Output = Pass> + Send;
}

/// A page with nothing live of its own. Its stream is held open all the
/// same, so the page learns at once when its session ended.
pub(super) struct Quiet;

impl Live for Quiet {
    fn open(_: &UiState, _: &AuthUser, _: &Shown) -> Self {
        Self
    }

    async fn draw(&mut self, _: &UiState, _: &Changed) -> Pass {
        Pass::Nothing
    }
}

/// A region as last sent.
pub(super) fn last() -> Last {
    Last::new()
}

/// What changes a page: the topics' signals. The first item is what moved
/// after the page was drawn; each after it gathers whatever came in since the
/// one before, as the watches it reads keep only the latest.
struct Sources {
    jobs: watch::Receiver<u64>,
    covers: watch::Receiver<u64>,
    boot: String,
    first: Option<Changed>,
}

impl Sources {
    fn new(s: &UiState, drawn: Option<&Seen>) -> Self {
        let mut jobs = s.jobs.subscribe();
        let mut covers = s.covers.subscribe();
        let now = Seen {
            boot: s.boot.0.to_string(),
            counts: [*jobs.borrow_and_update(), *covers.borrow_and_update()],
        };
        Self {
            jobs,
            covers,
            boot: now.boot.clone(),
            first: Some(Changed {
                topics: now.moved_since(drawn),
                seen: now,
            }),
        }
    }

    async fn next(&mut self, last: &Seen) -> Option<Changed> {
        if let Some(first) = self.first.take() {
            return Some(first);
        }
        tokio::select! {
            r = self.jobs.changed() => r.ok()?,
            r = self.covers.changed() => r.ok()?,
        }
        let now = Seen {
            boot: self.boot.clone(),
            counts: [
                *self.jobs.borrow_and_update(),
                *self.covers.borrow_and_update(),
            ],
        };
        Some(Changed {
            topics: now.moved_since(Some(last)),
            seen: now,
        })
    }
}

/// The live stream for the page `shown` names.
pub(super) async fn serve(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Signals(shown): Signals<Shown>,
) -> Response {
    let route = s.route(shown.url.split(['?', '#']).next().unwrap_or_default());
    if route == super::paths::LIBRARY && super::is_admin(&user) {
        run(s.clone(), library::Library::open(&s, &user, &shown), shown)
    } else {
        run(s.clone(), Quiet::open(&s, &user, &shown), shown)
    }
}

fn run<L: Live>(s: UiState, mut page: L, shown: Shown) -> Response {
    let sources = Sources::new(&s, shown.seen.as_ref());
    let start = sources
        .first
        .as_ref()
        .map(|c| c.seen.clone())
        .expect("the first change");
    let changes =
        futures_util::stream::unfold((sources, start), |(mut sources, last)| async move {
            let changed = sources.next(&last).await?;
            let seen = changed.seen.clone();
            Some((changed, (sources, seen)))
        });
    let events = async_stream::stream! {
        let mut changes = std::pin::pin!(paced(changes, Pace::default()));
        while let Some(changed) = changes.next().await {
            match page.draw(&s, &changed).await {
                Pass::Drew(events) => {
                    for e in events {
                        yield e;
                    }
                    yield super::signals(serde_json::json!({ "seen": changed.seen.signal() }));
                }
                Pass::Nothing => {}
                Pass::Failed => break,
            }
        }
    };
    askama_datastar::stream(events)
}

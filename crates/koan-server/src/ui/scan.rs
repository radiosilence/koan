//! Rescanning the library folders, for admins: the Library page's last
//! section. The scan is the same job `triggerScan` starts, so one started over
//! GraphQL shows here and the other way round, and the folder watcher's own
//! scans show beside them. The page keeps a stream open that shows each scan
//! as it starts, moves and ends, whoever started it.

use std::convert::Infallible;

use askama::Template;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use koan_core::auth::Role;

use super::{UiState, events, patch};
use crate::auth::AuthUser;
use crate::graphql::jobs::{Job, JobRegistry, JobState, REREAD, SCAN, WATCH, start_scan};
use crate::html::{Html, Render};

const KINDS: [&str; 3] = [SCAN, REREAD, WATCH];

fn allowed(user: &AuthUser) -> bool {
    user.role == Role::Admin
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "admins only").into_response()
}

#[derive(Template)]
#[template(path = "scan/status.html")]
struct Status {
    follow: bool,
    /// The line's colour and text; nothing before any scan.
    line: Option<(&'static str, String)>,
    /// A running scan's bar.
    bar: Option<Bar>,
}

/// How far a running scan has got, as its bar shows it.
enum Bar {
    /// The walk is still counting the files to read.
    Counting,
    Reading {
        done: u64,
        total: u64,
    },
}

/// What the current scan is doing, or the last did. On a page as it loads
/// (`on_load`) it asks to be followed; a stream's own patches never do, or
/// each would open another stream.
fn status(job: Option<&Job>, on_load: bool) -> Html {
    let line = job.map(|j| {
        let reason = j.reason.map(|r| format!(" ({r})")).unwrap_or_default();
        match j.state {
            JobState::Running => {
                let doing = if j.kind == REREAD {
                    "Reading every file again…".to_owned()
                } else {
                    format!("Scanning{reason}…")
                };
                let text = match j.done {
                    Some(_) => format!("{doing} {}", j.message),
                    None => doing,
                };
                ("text-muted", text)
            }
            JobState::Succeeded => {
                let what = if j.kind == REREAD {
                    "Last re-read".to_owned()
                } else {
                    format!("Last scan{reason}")
                };
                ("text-muted", format!("{what}: {}", j.message))
            }
            JobState::Failed => (
                "text-bad",
                format!("The scan failed{reason}: {}", j.message),
            ),
        }
    });
    // A scan that found nothing to read has no bar: it is about to end.
    let bar =
        job.filter(|j| j.state == JobState::Running)
            .and_then(|j| match j.done.zip(j.total) {
                None => Some(Bar::Counting),
                Some((_, 0)) => None,
                Some((done, total)) => Some(Bar::Reading { done, total }),
            });
    Status {
        follow: on_load,
        line,
        bar,
    }
    .html()
}

#[derive(Template)]
#[template(path = "scan/section.html")]
struct Section {
    status: Html,
}

/// The section, for an admin; nothing for anyone else.
pub(super) fn section(s: &UiState, user: &AuthUser) -> Html {
    if !allowed(user) {
        return Html::default();
    }
    Section {
        status: status(s.jobs.current(&KINDS).as_ref(), true),
    }
    .html()
}

#[derive(Template)]
#[template(path = "scan/error.html")]
struct Refused<'a> {
    error: &'a str,
}

/// Patches the status line as scans move: with `until_idle`, until none is
/// running, so a re-read queued behind a scan is followed once the scan ends;
/// otherwise for as long as the page is open, so a scan the folder watcher
/// starts shows when it starts. Waits on the registry's signal; the
/// keep-alive stops a proxy closing a stream that is quiet for a long stretch.
fn follow(jobs: JobRegistry, until_idle: bool) -> Response {
    let mut changed = jobs.subscribe();
    let stream = async_stream::stream! {
        let mut shown = Html::default();
        loop {
            let now = jobs.current(&KINDS);
            let html = status(now.as_ref(), false);
            if html != shown {
                yield Ok::<_, Infallible>(patch(&html, None));
                shown = html;
            }
            let idle = now.is_none_or(|j| j.state != JobState::Running);
            if (until_idle && idle) || changed.changed().await.is_err() {
                break;
            }
        }
    };
    let mut resp = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Start a scan, or join the one of the same kind running, and follow it.
/// Datastar posts its signals as JSON: `reread`.
pub(super) async fn start(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    body: Bytes,
) -> Response {
    if !allowed(&user) {
        return forbidden();
    }
    let reread = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("reread").and_then(|r| r.as_bool()))
        .unwrap_or(false);
    match start_scan(&s.jobs, s.pool.clone(), reread) {
        Ok(_) => follow(s.jobs.clone(), true),
        Err(e) => events(vec![patch(&Refused { error: &e }.html(), None)]),
    }
}

/// Follow the library's scans: what the page asks for as it opens.
pub(super) async fn watch(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    if !allowed(&user) {
        return forbidden();
    }
    follow(s.jobs.clone(), false)
}

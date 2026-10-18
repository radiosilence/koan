//! Rescanning the library folders, for admins: the Library page's last
//! section. The scan is the same job `triggerScan` starts, so one started over
//! GraphQL shows here and the other way round. Starting one, or opening the
//! page while one runs, opens a stream that follows it to its end.

use std::convert::Infallible;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use koan_core::auth::Role;

use super::pages::{COPY_ERROR, SUB};
use super::{UiState, events, patch};
use crate::auth::AuthUser;
use crate::graphql::jobs::{Job, JobRegistry, JobState, REREAD, SCAN, start_scan};
use crate::share::escape;

const KINDS: [&str; 2] = [SCAN, REREAD];

fn allowed(user: &AuthUser) -> bool {
    user.role == Role::Admin
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "admins only").into_response()
}

/// What the latest scan is doing, or has done. A running one asks to be
/// followed when the page loads with it.
fn status(job: Option<&Job>) -> String {
    let Some(j) = job else {
        return "<div id=scan-status></div>".into();
    };
    let reread = j.kind == REREAD;
    let (class, text, follow) = match j.state {
        JobState::Running => {
            let doing = if reread {
                "Reading every file again…"
            } else {
                "Scanning…"
            };
            let text = match j.done {
                Some(_) => format!("{doing} {}", j.message),
                None => doing.to_owned(),
            };
            ("text-muted", text, " data-init=\"@get('/library/scan')\"")
        }
        JobState::Succeeded => {
            let what = if reread { "Last re-read" } else { "Last scan" };
            ("text-muted", format!("{what}: {}", j.message), "")
        }
        JobState::Failed => ("text-bad", format!("The scan failed: {}", j.message), ""),
    };
    format!(
        "<div id=scan-status{follow}><p class=\"{class} text-control\" role=status>{}</p></div>",
        escape(&text)
    )
}

/// The section, for an admin; nothing for anyone else.
pub(super) fn section(s: &UiState, user: &AuthUser) -> String {
    if !allowed(user) {
        return String::new();
    }
    format!(
        "<h2>Rescan</h2><p class=\"{SUB}\">Look through the library folders for files added, \
changed or removed since the last scan. Reading every file again takes longer; it is for after \
an upgrade that reads more from tags.</p>\
<form class=\"mb-2 flex max-w-form flex-wrap items-center gap-3\" \
data-on:submit__prevent=\"@post('/library/scan')\">\
<label class=\"inline-flex items-center gap-1.5\"><input type=checkbox data-bind:reread>\
Read every file again</label>\
<button data-indicator:_scanning data-attr:disabled=\"$_scanning\">Rescan</button></form>{}",
        status(s.jobs.latest(&KINDS).as_ref())
    )
}

/// Patches the status line as `id` moves, until it ends. Waits on the
/// registry's signal; the keep-alive stops a proxy closing a stream that is
/// quiet for a long stretch of a scan.
fn follow(jobs: JobRegistry, id: Option<String>) -> Response {
    let mut changed = jobs.subscribe();
    let stream = async_stream::stream! {
        let mut shown = String::new();
        loop {
            let now = id.as_deref().and_then(|id| jobs.get(id));
            let html = status(now.as_ref());
            if html != shown {
                yield Ok::<_, Infallible>(patch(&html, None));
                shown = html;
            }
            if now.is_none_or(|j| j.state != JobState::Running) || changed.changed().await.is_err() {
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
    let pool = s.pool.clone();
    match start_scan(
        &s.jobs,
        move || pool.take().map_err(|e| e.to_string()),
        reread,
    ) {
        Ok(job) => follow(s.jobs.clone(), Some(job.id)),
        Err(e) => events(vec![patch(
            &format!(
                "<div id=scan-status><p class=\"{COPY_ERROR}\" role=alert>{}</p></div>",
                escape(&e)
            ),
            None,
        )]),
    }
}

/// Follow the latest scan: what a page opened while one runs asks for.
pub(super) async fn watch(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    if !allowed(&user) {
        return forbidden();
    }
    let latest = s.jobs.latest(&KINDS).map(|j| j.id);
    follow(s.jobs.clone(), latest)
}

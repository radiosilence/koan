//! Rescanning the library folders, for admins: the Library page's last
//! section. The scan is the same job `triggerScan` starts, so one started over
//! GraphQL shows here and the other way round; the page follows it as it runs
//! and shows its result line when it ends.

use std::convert::Infallible;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::sse::Sse;
use axum::response::{IntoResponse, Response};
use koan_core::auth::Role;

use super::pages::{COPY_ERROR, SUB};
use super::{UiState, events, patch};
use crate::auth::AuthUser;
use crate::graphql::jobs::{Job, JobState, start_scan};
use crate::share::escape;

pub(super) fn allowed(user: &AuthUser) -> bool {
    user.role == Role::Admin
}

/// What the latest scan is doing, or has done.
fn status(job: Option<&Job>) -> String {
    let line = match job {
        None => String::new(),
        Some(j) => {
            let (class, text) = match j.state {
                JobState::Running => ("text-muted", "Scanning…".to_owned()),
                JobState::Succeeded => ("text-muted", format!("Last scan: {}", j.message)),
                JobState::Failed => ("text-bad", format!("The scan failed: {}", j.message)),
            };
            format!(
                "<p class=\"{class} text-control\" role=status>{}</p>",
                escape(&text)
            )
        }
    };
    format!("<div id=scan-status>{line}</div>")
}

/// The section, for an admin; nothing for anyone else.
pub(super) fn section(s: &UiState, user: &AuthUser) -> String {
    if !allowed(user) {
        return String::new();
    }
    let latest = s.jobs.latest("scan");
    format!(
        "<h2>Rescan</h2><p class=\"{SUB}\">Look through the library folders for files added, \
changed or removed since the last scan. Reading every file again takes longer; it is for after \
an upgrade that reads more from tags.</p>\
<form class=\"mb-2 flex max-w-form flex-wrap items-center gap-3\" \
data-on:submit__prevent=\"@post('/library/scan')\">\
<label class=\"inline-flex items-center gap-1.5\"><input type=checkbox data-bind:reread>\
Read every file again</label><button>Rescan</button></form>{}",
        status(latest.as_ref())
    )
}

/// Start a scan (or join the one running) and follow it to its end. Datastar
/// posts its signals as JSON: `reread`.
pub(super) async fn start(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    body: Bytes,
) -> Response {
    if !allowed(&user) {
        return (StatusCode::FORBIDDEN, "admins only").into_response();
    }
    let reread = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("reread").and_then(|r| r.as_bool()))
        .unwrap_or(false);
    let mut changed = s.jobs.subscribe();
    let job = match start_scan(&s.jobs, s.pool.clone(), reread) {
        Ok(job) => job,
        Err(e) => {
            return events(vec![patch(
                &format!(
                    "<div id=scan-status><p class=\"{COPY_ERROR}\" role=alert>{}</p></div>",
                    escape(&e)
                ),
                None,
            )]);
        }
    };
    let jobs = s.jobs.clone();
    let stream = async_stream::stream! {
        let mut shown = String::new();
        loop {
            let now = jobs.get(&job.id);
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
    let mut resp = Sse::new(stream).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

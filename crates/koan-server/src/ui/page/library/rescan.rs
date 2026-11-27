//! Rescanning the library folders, for admins: the Library page's Rescan
//! section. The scan is the same job `triggerScan` starts, so one started over
//! GraphQL shows here and the other way round, and the folder watcher's own
//! scans show beside them. The page's stream draws each scan as it starts,
//! moves and ends, whoever started it.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use koan_kit_web as kit;

use crate::graphql::jobs::{Job, JobState, REREAD, SCAN, WATCH, start_scan};
use crate::html::{Html, Render};
use crate::ui::{UiState, flash_bad, nothing, paths};

const KINDS: [&str; 3] = [SCAN, REREAD, WATCH];

/// The region the stream redraws.
const ID: &str = "rescan";

/// What the current scan is doing, or the last did.
#[derive(Template)]
#[template(path = "library/rescan_status.html")]
struct Status {
    /// The line, in its tone; nothing before any scan.
    line: Option<kit::Status<'static>>,
    /// Why the last scan failed.
    failed: Option<String>,
    /// Only while a scan runs.
    bar: Option<kit::Meter<'static>>,
}

impl Status {
    fn id(&self) -> &'static str {
        ID
    }
}

fn of(job: Option<&Job>) -> Status {
    let mut status = Status {
        line: None,
        failed: None,
        bar: None,
    };
    let Some(j) = job else {
        return status;
    };
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
            status.line = Some(kit::Status::wait(text).busy(true));
            // A scan whose count came to nothing has no bar: it is about to end.
            status.bar = match j.done.zip(j.total) {
                None => Some(kit::Meter::waiting().label("Scan progress")),
                Some((_, 0)) => None,
                Some((done, total)) => {
                    Some(kit::Meter::new(done as f64, total as f64).label("Scan progress"))
                }
            };
        }
        JobState::Succeeded => {
            let what = if j.kind == REREAD {
                "Last re-read".to_owned()
            } else {
                format!("Last scan{reason}")
            };
            status.line = Some(kit::Status::ok(format!("{what}: {}", j.message)));
        }
        JobState::Failed => {
            status.failed = Some(format!("The scan failed{reason}: {}", j.message));
        }
    }
    status
}

/// The status region as the scans stand.
pub(in crate::ui) fn status(s: &UiState) -> Html {
    of(s.jobs.current(&KINDS).as_ref()).html()
}

#[derive(Template)]
#[template(path = "library/rescan.html")]
struct Section {
    status: Html,
}

impl Section {
    fn form(&self) -> kit::Form<'static> {
        kit::Form::post(paths::rescan())
    }
}

/// The section, as the page draws it.
pub(in crate::ui) fn section(s: &UiState) -> Html {
    Section { status: status(s) }.html()
}

#[derive(serde::Deserialize)]
pub(in crate::ui) struct Rescan {
    #[serde(default, deserialize_with = "crate::ui::parts::browse::flag")]
    reread: bool,
}

/// Start a scan, or join the one of the same kind running. The page's stream
/// draws it.
pub(in crate::ui) async fn start(
    State(s): State<UiState>,
    axum::Form(f): axum::Form<Rescan>,
) -> Response {
    match start_scan(&s.jobs, s.pool.clone(), f.reread) {
        Ok(_) => nothing(),
        Err(e) => flash_bad(&e),
    }
}

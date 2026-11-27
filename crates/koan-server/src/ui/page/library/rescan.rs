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

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

#[tokio::test]
async fn only_admins_see_and_start_a_rescan_which_reports_when_it_ends() {
    // The scan reads the library folders from config: the process's own,
    // which has none.
    koan_core::config::isolate_config_for_tests();
    let f = setup(true);
    let db = Database::open(f.state.pool.path()).unwrap();
    let boss = queries::auth::create_user(&db.conn, "boss", "sesame", Role::Admin).unwrap();
    let admin =
        auth::mint_access_token(&f.state.private_pem, boss, "boss", Role::Admin, 900).unwrap();
    let scan = |access: &str, signals: &'static str| {
        Request::post("/library/scan")
            .header(header::HOST, HOST)
            .header(header::COOKIE, format!("koan_access={access}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header("datastar-request", "true")
            .body(Body::from(signals))
            .unwrap()
    };
    let library = |access: &str| {
        get("/library")
            .header(header::COOKIE, format!("koan_access={access}"))
            .body(Body::empty())
            .unwrap()
    };

    // A wide screen's sidebar links the Library page for an admin only.
    let wide_link = "href=\"library\" data-nav=\"library\"";
    let r = send(&f.app, library(&access_token(&f.state))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains("/library/scan"), "{}", r.body);
    assert!(!r.body.contains(wide_link), "{}", r.body);
    let hidden_lists = "<ul class=\"wide:hidden\">";
    assert!(
        !r.body.contains(hidden_lists),
        "the lists are all a listener's page has: {}",
        r.body
    );
    let r = send(&f.app, scan(&access_token(&f.state), "{}")).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = send(&f.app, library(&admin)).await;
    assert!(r.body.contains("@post('library/scan')"), "{}", r.body);
    assert!(r.body.contains(wide_link), "{}", r.body);
    assert!(r.body.contains(hidden_lists), "{}", r.body);
    assert!(
        !r.body.contains("aria-label=\"Scan progress\""),
        "no bar while idle: {}",
        r.body
    );
    // The stream follows the scan to its end, so the whole body is its story.
    let r = send(&f.app, scan(&admin, r#"{"reread":false}"#)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body
            .contains("Last scan: 0 added, 0 updated, 0 removed, 0 unchanged"),
        "{}",
        r.body
    );
    let r = send(&f.app, scan(&admin, r#"{"reread":true}"#)).await;
    assert!(
        r.body.contains("Last re-read: 0 added"),
        "the box asks for a re-read: {}",
        r.body
    );
    let r = send(&f.app, library(&admin)).await;
    assert!(
        r.body.contains("Last re-read:"),
        "the page remembers it: {}",
        r.body
    );
    assert!(
        r.body.contains("data-init=\"@get('library/scan')\""),
        "the page follows scans still to come: {}",
        r.body
    );

    // A scan the folder watcher runs shows with what started it.
    let record = crate::graphql::jobs::record_watch(&f.jobs);
    record(&koan_core::helpers::WatchScan::Started("start-up"));
    let r = send(&f.app, library(&admin)).await;
    assert!(
        r.body.contains("aria-label=\"Scan progress\"></progress>")
            && !r.body.contains("max=\"1200\""),
        "indeterminate until the files are counted: {}",
        r.body
    );
    // Counted before any file is read, so the bar has its end from the start.
    record(&koan_core::helpers::WatchScan::Read {
        done: 0,
        total: 1200,
    });
    let r = send(&f.app, library(&admin)).await;
    assert!(r.body.contains("value=\"0\" max=\"1200\""), "{}", r.body);
    record(&koan_core::helpers::WatchScan::Read {
        done: 50,
        total: 1200,
    });
    let r = send(&f.app, library(&admin)).await;
    assert!(
        r.body
            .contains("Scanning (start-up)… 50 of 1200 files read"),
        "{}",
        r.body
    );
    assert!(
        r.body.contains("value=\"50\" max=\"1200\""),
        "the bar follows the count: {}",
        r.body
    );
}
}

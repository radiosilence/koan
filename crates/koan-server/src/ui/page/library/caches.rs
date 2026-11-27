//! What the server keeps beside the library, for admins: the Library page's
//! Caches section. The cover cache's size against its budget, by size, the
//! warm's progress and the last sweep; the download cache where the server
//! syncs from another; the library's counts and its last scan. The section
//! opens a stream that patches it as the warm, a sweep or a scan moves, at
//! most once a second, since a warm pass moves thousands of times.
//!
//! Transcodes are not listed: they are encoded as they stream and never kept.

use std::time::{Duration, SystemTime};

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use koan_kit_web as kit;

use crate::covers::{Report, Usage, WarmState};
use crate::graphql::jobs::{Job, JobState, REREAD, SCAN, WATCH};
use crate::html::{Html, Render};
use crate::ui::{UiState, open, paths};

/// The region the stream redraws.
const ID: &str = "caches";

#[derive(Template)]
#[template(path = "library/caches.html")]
struct Section {
    status: Html,
}

/// The section, around the status as it stands.
pub(in crate::ui) fn section(status: Html) -> Html {
    Section { status }.html()
}

#[derive(Template)]
#[template(path = "library/caches_status.html")]
struct Status {
    covers: Option<Covers>,
    downloads: Option<Downloads>,
    library: Option<Library>,
}

impl Status {
    fn id(&self) -> &'static str {
        ID
    }
}

struct Covers {
    used: String,
    budget: String,
    count: usize,
    misses: usize,
    pairs: kit::Pairs<'static>,
    progress: Option<(usize, usize)>,
    /// "Warm now", where it does anything.
    warm: Option<kit::Button<'static>>,
    /// "Clear covers", where there are any.
    clear: Option<kit::Button<'static>>,
}

struct Downloads {
    used: String,
    limit: Option<String>,
    tracks: i64,
}

struct Library {
    tracks: i64,
    albums: i64,
    artists: i64,
    scan: Option<String>,
}

/// The status as it stands: none when the library cannot be read. Lists the
/// cover directory: blocking.
pub(in crate::ui) fn status(s: &UiState) -> Option<Html> {
    let db = open(&s.pool)?;
    let usage = s.covers.usage(&db.conn);
    let stats = koan_core::db::queries::library_stats(&db.conn).ok();
    drop(db);
    let report = s.covers.report();
    let now = SystemTime::now();
    Some(
        Status {
            covers: Some(covers(&usage, &report, now)),
            downloads: stats.as_ref().and_then(downloads),
            library: stats.map(|st| Library {
                tracks: st.total_tracks,
                albums: st.total_albums,
                artists: st.total_artists,
                scan: last_scan(&s.jobs, now),
            }),
        }
        .html(),
    )
}

fn covers(u: &Usage, report: &Report, now: SystemTime) -> Covers {
    let warm = report.warm;
    let sizes = crate::covers::WARMED
        .map(|size| size.to_string())
        .join(" and ");
    let state = match warm.state {
        WarmState::Off => "Off: covers are made as they are asked for".to_owned(),
        WarmState::Running => format!("Making covers at {sizes}…"),
        WarmState::AtBudget => {
            "Paused: the cache is at its budget, so covers are made as asked".to_owned()
        }
        WarmState::Idle if u.unwarmed == 0 => format!("Every record has covers at {sizes}"),
        WarmState::Idle => "Idle".to_owned(),
    };
    let mut pairs = kit::Pairs::new();
    for size in &u.sizes {
        let without = if size.misses > 0 {
            format!(", {} without art", size.misses)
        } else {
            String::new()
        };
        pairs = pairs.pair(
            format!("{} px", size.size),
            format!("{} covers, {}{without}", size.covers, bytes(size.bytes)),
        );
    }
    let unwarmed = if u.unwarmed > 0 {
        format!("; {} records without them yet", u.unwarmed)
    } else {
        String::new()
    };
    pairs = pairs.pair("Warm", format!("{state}{unwarmed}"));
    if warm.state != WarmState::Running
        && let Some(t) = warm.finished
    {
        let made = match warm.made {
            0 => "nothing to make".to_owned(),
            1 => "1 record's covers made".to_owned(),
            n => format!("{n} records' covers made"),
        };
        pairs = pairs.pair("Last pass", format!("{}, {made}", ago(t, now)));
    }
    if let Some(t) = warm.next() {
        pairs = pairs.pair(
            "Next pass",
            format!("{}, or when the library changes", until(t, now)),
        );
    }
    pairs = pairs.pair(
        "Last sweep",
        match report.sweep {
            Some(sw) => format!(
                "{}, {} covers removed ({})",
                ago(sw.at, now),
                sw.removed,
                bytes(sw.bytes)
            ),
            None => "none needed since the server started".to_owned(),
        },
    );
    let warms = warm.state != WarmState::Off && warm.state != WarmState::Running;
    Covers {
        used: bytes(u.bytes),
        budget: bytes(u.budget),
        count: u.covers(),
        misses: u.misses(),
        pairs,
        progress: (warm.state == WarmState::Running).then_some((warm.done, warm.total)),
        warm: (warms && u.unwarmed > 0).then(|| {
            kit::Button::standard("Warm now").post(paths::caches_warm()).confirm(
                format!("Make covers now for the {} records without them?", u.unwarmed),
                "Warm now",
            )
        }),
        clear: (u.covers() > 0).then(|| {
            kit::Button::quiet("Clear covers")
                .bad()
                .post(paths::caches_clear())
                .confirm(
                    format!(
                        "Remove all {} kept covers ({})? Records show their covers again as they are asked for, and the warm makes the grid sizes again.",
                        u.covers(),
                        bytes(u.bytes)
                    ),
                    "Clear covers",
                )
        }),
    }
}

/// The download cache, where the server plays tracks from another server.
fn downloads(stats: &koan_core::db::queries::LibraryStats) -> Option<Downloads> {
    if stats.cached_tracks == 0 {
        return None;
    }
    // Kept as a running count once measured; nothing on a server measures it
    // otherwise.
    static MEASURED: std::sync::Once = std::sync::Once::new();
    let cfg = koan_core::config::Config::cached();
    MEASURED.call_once(|| {
        koan_core::helpers::measure_cache(&cfg);
    });
    Some(Downloads {
        used: bytes(koan_core::helpers::cache_bytes()),
        limit: cfg.cache_limit_bytes().map(bytes),
        tracks: stats.cached_tracks,
    })
}

fn last_scan(jobs: &crate::graphql::jobs::JobRegistry, now: SystemTime) -> Option<String> {
    let job: Job = jobs.last_finished(&[SCAN, REREAD, WATCH])?;
    let finished = job.finished?;
    let took = finished.duration_since(job.started).unwrap_or_default();
    let outcome = if job.state == JobState::Failed {
        "failed"
    } else {
        "finished"
    };
    Some(format!(
        "{outcome} {}, taking {}",
        ago(finished, now),
        duration(took)
    ))
}

/// Removes every kept cover. The page asked first, naming how many. The
/// stream follows the cache too, but the answer draws it at once, and closes
/// the question.
pub(in crate::ui) async fn clear(State(s): State<UiState>) -> Response {
    let state = s.clone();
    let html = crate::share::blocking(move || {
        state.covers.clear();
        status(&state)
    })
    .await;
    match html {
        Some(html) => askama_datastar::events([
            super::super::super::patch(&html),
            kit::Confirm::close().into(),
        ]),
        None => crate::ui::flash_bad("The library is unavailable."),
    }
}

/// Starts a warm pass now. The page's stream shows the pass.
pub(in crate::ui) async fn warm(State(s): State<UiState>) -> Response {
    s.covers.warm_now();
    askama_datastar::events([kit::Confirm::close().into()])
}

/// Bytes as people read them: binary units, as the budget is set.
pub(super) fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

fn duration(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0 => format!("{} ms", d.as_millis()),
        1..60 => format!("{secs} s"),
        60..3600 => format!("{} min {} s", secs / 60, secs % 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

/// How long ago `t` was, at the page's last patch.
fn ago(t: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(t).unwrap_or_default().as_secs();
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => plural(secs / 60, "minute", "ago"),
        3600..86_400 => plural(secs / 3600, "hour", "ago"),
        _ => plural(secs / 86_400, "day", "ago"),
    }
}

fn until(t: SystemTime, now: SystemTime) -> String {
    let secs = t.duration_since(now).unwrap_or_default().as_secs();
    match secs {
        0..60 => "in under a minute".to_owned(),
        60..3600 => format!("in {}", plural(secs / 60, "minute", "").trim_end()),
        _ => format!("in {}", plural(secs / 3600, "hour", "").trim_end()),
    }
}

fn plural(n: u64, unit: &str, after: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{n} {unit}{s} {after}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_times_read_plainly() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(4 * 1024 * 1024 * 1024), "4.0 GB");
        assert_eq!(bytes(200 * 1024 * 1024), "200 MB");
        let now = SystemTime::now();
        assert_eq!(ago(now - Duration::from_secs(5), now), "just now");
        assert_eq!(ago(now - Duration::from_secs(60), now), "1 minute ago");
        assert_eq!(ago(now - Duration::from_secs(7200), now), "2 hours ago");
        assert_eq!(until(now + Duration::from_secs(600), now), "in 10 minutes");
        assert_eq!(duration(Duration::from_secs(75)), "1 min 15 s");
    }
}

#[cfg(test)]
mod page_tests {
    use crate::ui::fixture::*;

#[tokio::test]
async fn only_admins_see_and_clear_the_caches() {
    let f = setup(true);
    let db = Database::open(f.state.pool.path()).unwrap();
    let boss = queries::auth::create_user(&db.conn, "boss", "sesame", Role::Admin).unwrap();
    let admin =
        auth::mint_access_token(&f.state.private_pem, boss, "boss", Role::Admin, 900).unwrap();
    let library = |access: &str| {
        get("/library")
            .header(header::COOKIE, format!("koan_access={access}"))
            .body(Body::empty())
            .unwrap()
    };
    let post = |path: &'static str, access: &str| {
        Request::post(path)
            .header(header::HOST, HOST)
            .header(header::COOKIE, format!("koan_access={access}"))
            .header("datastar-request", "true")
            .body(Body::empty())
            .unwrap()
    };
    let follows = "data-init=\"@get('library/caches')\"";

    let listener = access_token(&f.state);
    let r = send(&f.app, library(&listener)).await;
    assert!(!r.body.contains(follows), "{}", r.body);
    for path in ["/library/caches/clear", "/library/caches/warm"] {
        assert_eq!(
            send(&f.app, post(path, &listener)).await.status,
            StatusCode::FORBIDDEN
        );
    }

    let r = send(&f.app, library(&admin)).await;
    assert!(r.body.contains(follows), "{}", r.body);
    let r = send(&f.app, post("/library/caches/clear", &admin)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("of 4.0 GB, in 0 covers"), "{}", r.body);
    assert!(
        !r.body.contains("Clear covers"),
        "nothing left to clear: {}",
        r.body
    );
}
}

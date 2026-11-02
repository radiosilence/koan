//! What the server keeps beside the library, for admins: the Library page's
//! Caches section. The cover cache's size against its budget, by size, the
//! warm's progress and the last sweep; the download cache where the server
//! syncs from another; the library's counts and its last scan. The section
//! opens a stream that patches it as the warm, a sweep or a scan moves, at
//! most once a second, since a warm pass moves thousands of times.
//!
//! Transcodes are not listed: they are encoded as they stream and never kept.

use std::convert::Infallible;
use std::time::{Duration, SystemTime};

use askama::Template;
use axum::Extension;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};

use super::{UiState, events, patch};
use crate::auth::AuthUser;
use crate::covers::{Report, Usage, WarmState};
use crate::graphql::jobs::{Job, JobState, REREAD, SCAN, WATCH};
use crate::html::{Html, Render};

/// The shortest gap between two patches of one stream.
const PATCH_EVERY: Duration = Duration::from_secs(1);

#[derive(Template)]
#[template(path = "caches/section.html")]
struct Section;

/// The section, for an admin; nothing for anyone else. Empty until its
/// stream's first patch, so the page does not wait on a directory listing.
pub(super) fn section(user: &AuthUser) -> Html {
    if !super::scan::allowed(user) {
        return Html::default();
    }
    Section.html()
}

#[derive(Template)]
#[template(path = "caches/status.html")]
struct Status {
    covers: Option<Covers>,
    downloads: Option<Downloads>,
    library: Option<Library>,
}

struct Covers {
    used: String,
    budget: String,
    /// Size, covers, bytes, misses.
    sizes: Vec<(u32, usize, String, usize)>,
    misses: usize,
    count: usize,
    unwarmed: usize,
    /// What the warm is doing, and how far it has got while running.
    warm: String,
    progress: Option<(usize, usize)>,
    /// The last pass and the next, when known.
    last: Option<String>,
    next: Option<String>,
    sweep: Option<String>,
    /// Whether "Warm now" does anything.
    warms: bool,
    /// The clear's confirmation: what it removes.
    clear: String,
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

/// The section as it stands. Lists the cover directory: blocking.
fn status(s: &UiState) -> Html {
    let db = super::open(&s.pool);
    let usage = db.as_ref().map(|db| s.covers.usage(&db.conn));
    let stats = db
        .as_ref()
        .and_then(|db| koan_core::db::queries::library_stats(&db.conn).ok());
    drop(db);
    let report = s.covers.report();
    let now = SystemTime::now();
    Status {
        covers: usage.map(|u| covers(&u, &report, now)),
        downloads: stats.as_ref().and_then(downloads),
        library: stats.map(|st| Library {
            tracks: st.total_tracks,
            albums: st.total_albums,
            artists: st.total_artists,
            scan: last_scan(&s.jobs, now),
        }),
    }
    .html()
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
    let last = warm
        .finished
        .filter(|_| warm.state != WarmState::Running)
        .map(|t| {
            let made = match warm.made {
                0 => "nothing to make".to_owned(),
                1 => "1 record's covers made".to_owned(),
                n => format!("{n} records' covers made"),
            };
            format!("{}, {made}", ago(t, now))
        });
    let sweep = report.sweep.map(|sw| {
        format!(
            "{}, {} covers removed ({})",
            ago(sw.at, now),
            sw.removed,
            bytes(sw.bytes)
        )
    });
    Covers {
        used: bytes(u.bytes),
        budget: bytes(u.budget),
        sizes: u
            .sizes
            .iter()
            .map(|s| (s.size, s.covers, bytes(s.bytes), s.misses))
            .collect(),
        misses: u.misses(),
        count: u.covers(),
        unwarmed: u.unwarmed,
        warm: state,
        progress: (warm.state == WarmState::Running).then_some((warm.done, warm.total)),
        last,
        next: warm.next().map(|t| until(t, now)),
        sweep,
        warms: warm.state != WarmState::Off && warm.state != WarmState::Running,
        clear: format!(
            "Remove all {} kept covers ({})? Records show their covers again as they are asked for, and the warm makes the grid sizes again.",
            u.covers(),
            bytes(u.bytes)
        ),
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

/// Patches the section as the covers, the jobs or the library move, at most
/// once every `PATCH_EVERY`, for as long as the page is open.
pub(super) async fn watch(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    if !super::scan::allowed(&user) {
        return (StatusCode::FORBIDDEN, "admins only").into_response();
    }
    let mut covers = s.covers.subscribe();
    let mut jobs = s.jobs.subscribe();
    let stream = async_stream::stream! {
        let mut shown = Html::default();
        loop {
            let state = s.clone();
            let Ok(html) = tokio::task::spawn_blocking(move || status(&state)).await else {
                break;
            };
            if html != shown {
                yield Ok::<_, Infallible>(patch(&html, None));
                shown = html;
            }
            tokio::time::sleep(PATCH_EVERY).await;
            let moved = tokio::select! {
                r = covers.changed() => r,
                r = jobs.changed() => r,
            };
            if moved.is_err() {
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

/// Removes every kept cover. The page asked first, naming how many.
pub(super) async fn clear(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    if !super::scan::allowed(&user) {
        return (StatusCode::FORBIDDEN, "admins only").into_response();
    }
    let state = s.clone();
    let html = tokio::task::spawn_blocking(move || {
        state.covers.clear();
        status(&state)
    })
    .await
    .unwrap_or_default();
    events(vec![patch(&html, None)])
}

/// Starts a warm pass now.
pub(super) async fn warm(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    if !super::scan::allowed(&user) {
        return (StatusCode::FORBIDDEN, "admins only").into_response();
    }
    s.covers.warm_now();
    // The section's own stream shows the pass.
    events(Vec::new())
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

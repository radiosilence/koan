//! What the signed-in account has listened to, most recent first.
//!
//! A list of plays, not of tracks: a record played three times is three rows.
//! The only grouping is the day, in the browser's own time zone, which the
//! UI's script leaves in the `koan_tz` cookie as minutes east of UTC.

use std::fmt::Write as _;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::{DateTime, Datelike, FixedOffset, TimeZone, Utc};
use koan_core::db::queries::{self, PlayHistoryRow};

use super::favourite::Hearts;
use super::pages::{EMPTY, ERROR, cover_url, track_versions, unavailable};
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::share::{blocking, duration, escape};

/// Plays on one page; older ones are a link away.
const PAGE: u32 = 300;

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(super) struct Paging {
    page: u32,
}

/// The browser's zone, as the script left it; UTC until it has.
fn zone(headers: &HeaderMap) -> FixedOffset {
    super::cookie(headers, "koan_tz")
        .and_then(|m| m.parse::<i32>().ok())
        .filter(|m| m.abs() <= 18 * 60)
        .and_then(|m| FixedOffset::east_opt(m * 60))
        .unwrap_or(FixedOffset::east_opt(0).expect("zero is an offset"))
}

/// "Today", "Yesterday", the weekday within the week, then the date.
fn day_label(day: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> String {
    let ago = now
        .date_naive()
        .signed_duration_since(day.date_naive())
        .num_days();
    match ago {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == now.year() => day.format("%A %-d %B").to_string(),
        _ => day.format("%-d %B %Y").to_string(),
    }
}

fn plays(
    rows: &[PlayHistoryRow],
    versions: &super::pages::Versions,
    zone: FixedOffset,
    hearts: Option<&Hearts>,
) -> String {
    let now = Utc::now().with_timezone(&zone);
    let mut out = String::new();
    let mut day = None;
    for p in rows {
        let at = zone.timestamp_opt(p.played_at, 0).single().unwrap_or(now);
        if day != Some(at.date_naive()) {
            day = Some(at.date_naive());
            let _ = write!(out, "<li class=\"disc\">{}</li>", day_label(at, now));
        }
        let t = &p.track;
        let _ = write!(
            out,
            "<li tabindex=0 data-id={id} data-dur={secs} data-title=\"{title}\" data-artist=\"{artist}\" \
data-album=\"{album}\" data-album-id={album_id} data-cover=\"{cover}\">\
<input type=checkbox class=\"flex-none\" value={play} aria-label=\"Select this play\">\
<span class=\"n w-auto\">{time}</span><span class=\"t\">{title}<small>{artist} · {album}</small></span>\
<span class=\"d\">{dur}</span>{heart}\
<button class=\"quiet\" data-act=add aria-label=\"Add to queue\" title=\"Add to queue\">+</button></li>",
            id = t.id,
            play = p.id,
            secs = t.duration_ms.unwrap_or(0) / 1000,
            title = escape(&t.title),
            artist = escape(&t.artist_name),
            album = escape(&t.album_title),
            album_id = t.album_id.unwrap_or(0),
            cover = t
                .album_id
                .map(|a| cover_url(a, crate::covers::LARGE, versions))
                .unwrap_or_default(),
            time = at.format("%H:%M"),
            dur = duration(t.duration_ms),
            heart = hearts.map(|h| h.track(t.id)).unwrap_or_default(),
        );
    }
    out
}

/// One page of plays, read for one account.
struct Plays {
    rows: Vec<PlayHistoryRow>,
    /// There are older ones.
    more: bool,
    versions: super::pages::Versions,
    hearts: Option<Hearts>,
}

fn read(s: &UiState, user: &AuthUser, page: u32) -> Option<Plays> {
    let db = open(&s.pool)?;
    let mut rows = queries::play_history_with_tracks(
        &db.conn,
        user.user_id,
        None,
        Some(PAGE + 1),
        page * PAGE,
    )
    .ok()?;
    let more = rows.len() > PAGE as usize;
    rows.truncate(PAGE as usize);
    let tracks: Vec<_> = rows.iter().map(|r| r.track.clone()).collect();
    let versions = track_versions(&db.conn, &tracks);
    let hearts = Hearts::load(&db.conn, user);
    Some(Plays {
        rows,
        more,
        versions,
        hearts,
    })
}

/// The list and its paging, as one element the forget action can replace.
fn list(p: &Plays, page: u32, zone: FixedOffset) -> String {
    let (rows, more, versions) = (&p.rows, p.more, &p.versions);
    if rows.is_empty() && page == 0 {
        return format!(
            "<div id=history><p class=\"{EMPTY}\">Nothing played yet.</p>\
<p class=\"{EMPTY}\">What you play here, in the kōan apps or in a Subsonic app signed in as you is listed \
by day.</p></div>"
        );
    }
    let mut nav = Vec::new();
    if page > 0 {
        nav.push(format!(
            "<a href=\"/history{}\">Newer</a>",
            if page == 1 {
                String::new()
            } else {
                format!("?page={}", page - 1)
            }
        ));
    }
    if more {
        nav.push(format!(
            "<a href=\"/history?page={}\">Earlier</a>",
            page + 1
        ));
    }
    let nav = if nav.is_empty() {
        String::new()
    } else {
        format!("<p class=\"mt-5 flex gap-4\">{}</p>", nav.join(""))
    };
    format!(
        "<div id=history><ol class=\"tracks\" data-context=one>{}</ol>{nav}</div>",
        plays(rows, versions, zone, p.hearts.as_ref())
    )
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(paging): Query<Paging>,
    headers: HeaderMap,
) -> Response {
    let (st, page, who) = (s.clone(), paging.page, user.clone());
    let Some(plays) = blocking(move || read(&st, &who, page)).await else {
        return unavailable();
    };
    // Picked plays collect in `$forget`, read off the boxes ticked whenever
    // one changes; the bar to forget them shows once there is one.
    let inner = format!(
        "<div data-signals:forget=\"[]\" \
data-on:change=\"$forget = [...el.querySelectorAll('#history input:checked')].map(i => i.value)\"><div class=\"flex flex-wrap items-baseline justify-between gap-2\">\
<h1>History</h1><div class=\"flex items-center gap-2\" data-show=\"$forget.length > 0\">\
<span class=\"text-meta text-muted\" data-text=\"$forget.length + ' selected'\"></span>\
<button data-on:click=\"el.closest('.page').querySelectorAll('#history input:checked').forEach(i => i.checked = false); $forget = []\">Deselect</button>\
<button class=\"primary\" data-indicator:_forgetting data-attr:disabled=\"$_forgetting\" \
data-on:click=\"@post('/history/forget?page={page}')\">Forget</button></div></div>\
<div id=history-result></div>{}</div>",
        list(&plays, page, zone(&headers))
    );
    super::pages::respond(&s, &headers, &user, "History", &inner)
}

/// Forget the plays picked, the account's own only, and show the page again.
pub(super) async fn forget(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(paging): Query<Paging>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ids: Vec<i64> = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("forget").cloned())
        .and_then(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
        .collect();
    let page = paging.page;
    let found = blocking(move || {
        let db = open(&s.pool)?;
        queries::delete_plays(&db.conn, user.user_id, &ids).ok()?;
        drop(db);
        read(&s, &user, page)
    })
    .await;
    match found {
        Some(plays) => events(vec![
            patch(&list(&plays, page, zone(&headers)), None),
            patch("<div id=history-result></div>", None),
            axum::response::sse::Event::default()
                .event("datastar-patch-signals")
                .data("signals {\"forget\":[]}"),
        ]),
        None => events(vec![patch(
            &format!(
                "<div id=history-result class=\"mt-3 {ERROR}\" role=alert>Those plays could not be forgotten.</div>"
            ),
            None,
        )]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_named_as_near_as_they_are() {
        let zone = FixedOffset::east_opt(3600).unwrap();
        let now = zone.with_ymd_and_hms(2026, 10, 5, 0, 30, 0).unwrap();
        let at = |d, h| zone.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap();
        assert_eq!(day_label(at(5, 0), now), "Today");
        assert_eq!(day_label(at(4, 23), now), "Yesterday");
        assert_eq!(day_label(at(1, 12), now), "Thursday");
        let earlier = zone.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap();
        assert_eq!(day_label(earlier, now), "Monday 28 September");
        let last_year = zone.with_ymd_and_hms(2025, 12, 31, 9, 0, 0).unwrap();
        assert_eq!(day_label(last_year, now), "31 December 2025");
    }

    #[test]
    fn the_zone_is_the_browsers_and_utc_without_one() {
        let mut headers = HeaderMap::new();
        assert_eq!(zone(&headers).local_minus_utc(), 0);
        headers.insert(
            axum::http::header::COOKIE,
            "a=b; koan_tz=-300".parse().unwrap(),
        );
        assert_eq!(zone(&headers).local_minus_utc(), -300 * 60);
        headers.insert(axum::http::header::COOKIE, "koan_tz=99999".parse().unwrap());
        assert_eq!(zone(&headers).local_minus_utc(), 0, "nonsense is ignored");
    }
}

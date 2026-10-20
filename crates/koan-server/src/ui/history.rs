//! What the signed-in account has listened to, most recent first.
//!
//! A list of plays, not of tracks: a record played three times is three rows.
//! The only grouping is the day, in the browser's own time zone, which the
//! UI's script leaves in the `koan_tz` cookie as minutes east of UTC.

use askama::Template;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::{DateTime, Datelike, FixedOffset, TimeZone, Utc};
use koan_core::db::queries::{self, PlayHistoryRow};

use super::favourite::Hearts;
use super::pages::{Credits, ERROR, Failed, TrackData, track_versions, unavailable};
use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;

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

/// One play's row, with the day's heading before it where the day changes.
struct Play<'a> {
    day: Option<String>,
    play: i64,
    time: String,
    row: TrackData<'a>,
}

fn plays<'a>(
    rows: &'a [PlayHistoryRow],
    versions: &super::pages::Versions,
    credits: &Credits,
    zone: FixedOffset,
    hearts: Option<&'a Hearts>,
) -> Vec<Play<'a>> {
    let now = Utc::now().with_timezone(&zone);
    let mut day = None;
    rows.iter()
        .map(|p| {
            let at = zone.timestamp_opt(p.played_at, 0).single().unwrap_or(now);
            let heading = (day != Some(at.date_naive())).then(|| {
                day = Some(at.date_naive());
                day_label(at, now)
            });
            Play {
                day: heading,
                play: p.id,
                time: at.format("%H:%M").to_string(),
                row: TrackData::new(&p.track, versions, credits, hearts, true, true),
            }
        })
        .collect()
}

/// One page of plays, read for one account.
struct Plays {
    rows: Vec<PlayHistoryRow>,
    /// There are older ones.
    more: bool,
    versions: super::pages::Versions,
    credits: Credits,
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
    let credits = Credits::load(&db.conn, &tracks, &[]);
    let hearts = Hearts::load(&db.conn, user);
    Some(Plays {
        rows,
        more,
        versions,
        credits,
        hearts,
    })
}

#[derive(Template)]
#[template(path = "history/list.html")]
struct List<'a> {
    plays: Vec<Play<'a>>,
    page: u32,
    /// There are older ones.
    more: bool,
}

/// The list and its paging, as one element the forget action can replace.
fn list(p: &Plays, page: u32, zone: FixedOffset) -> Html {
    List {
        plays: plays(&p.rows, &p.versions, &p.credits, zone, p.hearts.as_ref()),
        page,
        more: p.more,
    }
    .html()
}

#[derive(Template)]
#[template(path = "pages/history.html")]
struct HistoryPage {
    page: u32,
    list: Html,
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
    let inner = HistoryPage {
        page,
        list: list(&plays, page, zone(&headers)),
    }
    .html();
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
        queries::forget_shared_entries(&db.conn, user.user_id, &ids).ok()?;
        use koan_core::smart::Field;
        crate::clients::smart_activity(&db, user.user_id, &[Field::PlayCount, Field::LastPlayed]);
        drop(db);
        // The account's linked devices forget them too.
        crate::clients::registry().broadcast(
            Some(&user.username),
            koan_core::remote::link::LinkCommand::HistoryChanged,
        );
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
        None => {
            let html = Failed {
                id: "history-result",
                class: &format!("mt-3 {ERROR}"),
                message: "Those plays could not be forgotten.",
            }
            .html();
            events(vec![patch(&html, None)])
        }
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

//! What the signed-in account has listened to, most recent first.
//!
//! A list of plays, not of tracks: a record played three times is three rows.
//! The only grouping is the day, in the browser's own time zone, which the
//! UI's script leaves in the `koan_tz` cookie as minutes east of UTC.

use askama::Template;
use askama_datastar::View;
use axum::Extension;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::{DateTime, Datelike, FixedOffset, TimeZone, Utc};
use koan_core::db::queries::{self, PlayHistoryRow};
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::parts::covers::Versions;
use crate::ui::parts::credit::Credits;
use crate::ui::parts::heart::Hearts;
use crate::ui::parts::track::{Lead, Row, Rows};
use crate::ui::{Logged, Sent, UiState, flash_bad, open, patch, paths, signals};

/// Plays on one page.
const PAGE: u32 = 300;

/// The region the forget action redraws.
const ID: &str = "history";

pub(in crate::ui) struct History;

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(in crate::ui) struct Paging {
    page: u32,
}

fn zone(headers: &HeaderMap) -> FixedOffset {
    crate::ui::cookie(headers, "koan_tz")
        .and_then(|m| m.parse::<i32>().ok())
        .filter(|m| m.abs() <= 18 * 60)
        .and_then(|m| FixedOffset::east_opt(m * 60))
        .unwrap_or(FixedOffset::east_opt(0).expect("zero is an offset"))
}

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

/// The plays on one page, grouped by day, and links to newer and earlier
/// pages: the region the forget action replaces. Blocking.
fn list(s: &UiState, user: &AuthUser, page: u32, zone: FixedOffset) -> Option<Html> {
    let db = open(&s.pool)?;
    let mut plays: Vec<PlayHistoryRow> = queries::play_history_with_tracks(
        &db.conn,
        user.user_id,
        None,
        Some(PAGE + 1),
        page * PAGE,
    )
    .logged()?;
    let more = plays.len() > PAGE as usize;
    plays.truncate(PAGE as usize);
    let tracks: Vec<_> = plays.iter().map(|r| r.track.clone()).collect();
    let versions = Versions::of_tracks(&db.conn, &tracks);
    let credits = Credits::load(&db.conn, &tracks, &[]);
    let hearts = Hearts::load(&db.conn, user);
    drop(db);
    let rows = Rows {
        versions: &versions,
        credits: &credits,
        hearts: hearts.as_ref(),
    };
    let now = Utc::now().with_timezone(&zone);
    let mut day = None;
    let drawn = plays
        .iter()
        .map(|p| {
            let at = zone.timestamp_opt(p.played_at, 0).single().unwrap_or(now);
            let heading = (day != Some(at.date_naive())).then(|| {
                day = Some(at.date_naive());
                day_label(at, now)
            });
            let lead = Lead::Played {
                time: at.format("%H:%M").to_string(),
                play: p.id,
            };
            (heading, Row::new(&p.track, lead, &rows, true, true).html())
        })
        .collect();
    Some(
        List {
            plays: drawn,
            newer: match page {
                0 => None,
                n => Some(paths::history_page(n - 1)),
            },
            earlier: more.then(|| paths::history_page(page + 1)),
            first: page == 0,
        }
        .html(),
    )
}

#[derive(Template)]
#[template(path = "history/list.html")]
struct List {
    /// Each play's row, after its day's heading where the day changes.
    plays: Vec<(Option<String>, Html)>,
    newer: Option<String>,
    earlier: Option<String>,
    first: bool,
}

impl List {
    fn id(&self) -> &'static str {
        ID
    }
}

#[derive(Template)]
#[template(path = "history/page.html")]
struct HistoryPage {
    page: u32,
    list: Html,
}

impl HistoryPage {
    fn forget(&self) -> kit::Button<'static> {
        kit::Button::standard("Forget")
            .bad()
            .data("forget", "picked")
            .post(paths::history_forget(self.page))
    }

    fn deselect(&self) -> kit::Button<'static> {
        kit::Button::quiet("Deselect").datastar(
            "on:click",
            "document.querySelectorAll('#history input:checked').forEach(i => i.checked = false), $forget = []",
        )
    }
}

impl Page for History {
    const SECTION: Section = Section::History;
    type Args = (View<Paging>, HeaderMap);

    async fn body(
        s: &UiState,
        user: &AuthUser,
        (View(paging), headers): (View<Paging>, HeaderMap),
    ) -> Result<Body, Refusal> {
        let (st, page, who, zone) = (s.clone(), paging.page, user.clone(), zone(&headers));
        let list = blocking(move || list(&st, &who, page, zone))
            .await
            .ok_or(Refusal::Unavailable)?;
        Ok(Body::of("History", &HistoryPage { page, list }).signal("forget", Vec::<i64>::new()))
    }
}

#[derive(serde::Deserialize)]
pub(in crate::ui) struct Forget {
    forget: Vec<i64>,
}

/// Forget the plays picked, and draw the page of plays again without them.
pub(in crate::ui) async fn forget(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(paging): Query<Paging>,
    headers: HeaderMap,
    Sent(Forget { forget }): Sent<Forget>,
) -> Response {
    let (page, zone) = (paging.page, zone(&headers));
    let found = blocking(move || {
        let db = open(&s.pool)?;
        queries::forget_shared_entries(&db.conn, user.user_id, &forget).logged()?;
        use koan_core::smart::Field;
        crate::clients::smart_activity(&db, user.user_id, &[Field::PlayCount, Field::LastPlayed]);
        drop(db);
        crate::clients::registry().broadcast(
            Some(&user.username),
            koan_core::remote::link::LinkCommand::HistoryChanged,
        );
        list(&s, &user, page, zone)
    })
    .await;
    match found {
        Some(list) => {
            askama_datastar::events([patch(&list), signals(serde_json::json!({ "forget": [] }))])
        }
        None => flash_bad("Those plays could not be forgotten."),
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

#[cfg(test)]
mod page_tests {
    use crate::ui::fixture::*;

#[tokio::test]
async fn history_lists_the_callers_own_plays_by_day_and_forgets_only_those() {
    let f = setup(true);
    let now = chrono::Utc::now().timestamp();
    let (mine, theirs) = {
        let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
        let bob = queries::auth::create_user(&db.conn, "bob", "hunter3", Role::User).unwrap();
        let mine = queries::record_play_at(&db.conn, 1, f.track_id, now, None, "local").unwrap();
        queries::record_play_at(&db.conn, 1, f.track_id, now - 3 * 86_400, None, "local").unwrap();
        let theirs =
            queries::record_play_at(&db.conn, bob, f.track_id, now, None, "local").unwrap();
        (mine, theirs)
    };
    let page = || {
        authed(&f.state, "/history")
            .header(header::COOKIE, "koan_tz=0")
            .body(Body::empty())
            .unwrap()
    };
    let r = send(&f.app, page()).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains(">Today</li>"), "grouped by day");
    assert_eq!(
        r.body.matches("type=\"checkbox\"").count(),
        2,
        "alice's two plays, not bob's"
    );
    assert!(
        r.body.contains(&format!("value=\"{mine}\""))
            && !r.body.contains(&format!("value=\"{theirs}\""))
    );

    let forget = |ids: &str| {
        Request::post("/history/forget")
            .header(header::HOST, HOST)
            .header(
                header::COOKIE,
                format!("koan_access={}", access_token(&f.state)),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .header("datastar-request", "true")
            .body(Body::from(format!("{{\"forget\":{ids}}}")))
            .unwrap()
    };
    // Another account's play, named by id, is left alone.
    let r = send(&f.app, forget(&format!("[\"{theirs}\"]"))).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(&f.app, forget(&format!("[\"{mine}\"]"))).await;
    assert!(r.body.contains("datastar-patch-elements") && r.body.contains("\"forget\":[]"));
    let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
    let left: Vec<i64> = db
        .conn
        .prepare("SELECT id FROM play_history ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .flatten()
        .collect();
    assert!(!left.contains(&mine), "alice's play is forgotten");
    assert!(left.contains(&theirs), "bob's is not");
    assert_eq!(left.len(), 2);
    // Recorded, so the account's devices forget it too; bob's is not.
    let forgotten: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM play_history_forgotten", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(forgotten, 1);
}
}

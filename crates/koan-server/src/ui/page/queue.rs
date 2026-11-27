//! Queue: what plays next. The queue lives in the browser, with the player;
//! the player's script asks for its rows whenever it changes and the page is
//! shown, and the server draws them with the same row as every other list.

use askama::Template;
use axum::Extension;
use axum::extract::State;
use axum::response::Response;
use koan_core::db::queries;
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::parts::covers::Versions;
use crate::ui::parts::credit::Credits;
use crate::ui::parts::heart::Hearts;
use crate::ui::parts::track::{Lead, Row, Rows};
use crate::ui::parts::{Scrub, Transport};
use crate::ui::{Logged, Sent, UiState, answer, flash_bad, open, paths};

/// The id of the list the rows go in.
const ID: &str = "queue";

pub(in crate::ui) struct Queue;

#[derive(Template)]
#[template(path = "queue/page.html")]
struct QueuePage {
    list: Html,
}

impl QueuePage {
    fn transport(&self) -> Html {
        Transport::page()
    }

    fn scrub(&self) -> Html {
        Scrub::page()
    }
}

/// The queue's rows, for the tracks the player holds, in its order. `drawn`
/// names them, so the player's script asks again only when its queue differs.
#[derive(Template)]
#[template(path = "queue/list.html")]
struct List {
    drawn: String,
    rows: Html,
    empty: bool,
}

impl List {
    fn id(&self) -> &'static str {
        ID
    }

    fn rows_url(&self) -> &'static str {
        paths::queue_rows()
    }
}

impl Page for Queue {
    const SECTION: Section = Section::Queue;
    type Args = ();

    async fn body(_: &UiState, _: &AuthUser, (): ()) -> Result<Body, Refusal> {
        // Drawn empty and unnamed: the player's script asks for the rows of
        // the queue it holds as soon as the list is on the page.
        let list = List {
            drawn: "-".to_owned(),
            rows: Html::default(),
            empty: false,
        }
        .html();
        Ok(Body::of("Queue", &QueuePage { list }))
    }
}

#[derive(serde::Deserialize)]
pub(in crate::ui) struct Ids {
    ids: Vec<i64>,
}

/// The rows for the tracks the player's script sends, in its order. A track
/// gone from the library draws no row, and the rest keep their places. The
/// whole queue is drawn, however long: `drawn` names exactly the tracks asked
/// for, so the player's script, which compares it with the queue it holds,
/// never asks again for a queue already drawn.
pub(in crate::ui) async fn rows(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Sent(Ids { ids }): Sent<Ids>,
) -> Response {
    let drawn = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let list = blocking(move || {
        let db = open(&s.pool)?;
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        let tracks = queries::tracks_by_ids(&db.conn, &unique).logged()?;
        let by_id: std::collections::HashMap<i64, &queries::TrackRow> =
            tracks.iter().map(|t| (t.id, t)).collect();
        let versions = Versions::of_tracks(&db.conn, &tracks);
        let credits = Credits::load(&db.conn, &tracks, &[]);
        let hearts = Hearts::load(&db.conn, &user);
        let rows = Rows {
            versions: &versions,
            credits: &credits,
            hearts: hearts.as_ref(),
        };
        let html: Html = ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| by_id.get(id).map(|t| (i, *t)))
            .map(|(i, t)| Row::new(t, Lead::Queued(i), &rows, true, true).html())
            .collect();
        Some(
            List {
                empty: ids.is_empty(),
                drawn,
                rows: html,
            }
            .html(),
        )
    })
    .await;
    match list {
        Some(list) => answer([list]),
        None => flash_bad("The library is unavailable."),
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    /// However long the player's queue, the rows drawn are named by all of
    /// it, so the script, comparing that with the queue it holds, does not
    /// ask again.
    #[tokio::test]
    async fn a_long_queue_is_drawn_whole_and_named_by_all_of_it() {
        let f = setup(true);
        let ids = vec![f.track_id; 6000];
        let body = serde_json::json!({ "ids": ids }).to_string();
        let r = send(
            &f.app,
            Request::post("/queue/rows")
                .header(header::HOST, HOST)
                .header(
                    header::COOKIE,
                    format!("koan_access={}", access_token(&f.state)),
                )
                .header("datastar-request", "true")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        let drawn = r
            .body
            .split("data-drawn=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap();
        assert_eq!(drawn.split(',').count(), 6000);
        assert_eq!(r.body.matches("data-q=\"").count(), 6000);
    }
}

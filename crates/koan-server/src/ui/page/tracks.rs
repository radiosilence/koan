//! Tracks: every track the filters let through, a page at a time. Picking
//! one plays on through the page.

use askama::Template;
use askama_datastar::View;
use koan_core::shelves;
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::ui::parts::browse::{self, Browse, Kind, Toolbar};
use crate::ui::parts::count;
use crate::ui::parts::covers::Versions;
use crate::ui::parts::credit::Credits;
use crate::ui::parts::heart::Hearts;
use crate::ui::parts::track::{Lead, Row, Rows};
use crate::ui::{Logged, UiState, open};

/// Tracks on one page of the track browser.
const PAGE: u32 = 200;

pub(in crate::ui) struct Tracks;

#[derive(Template)]
#[template(path = "tracks/page.html")]
struct TracksPage {
    total: String,
    toolbar: Html,
    rows: Html,
    empty: bool,
    previous: Option<String>,
    next: Option<String>,
}

impl Page for Tracks {
    const SECTION: Section = Section::Tracks;
    type Args = View<Browse>;

    async fn body(s: &UiState, user: &AuthUser, View(b): View<Browse>) -> Result<Body, Refusal> {
        let page = b.page;
        let (st, bb, who) = (s.clone(), b.clone(), user.clone());
        let (rows, empty, total, options) = blocking(move || {
            let db = open(&st.pool)?;
            let listing = bb.tracks(who.user_id, shelves::now());
            let total = listing.count(&db.conn).logged()?;
            let tracks = listing.page(&db.conn, PAGE, page * PAGE).logged()?;
            let versions = Versions::of_tracks(&db.conn, &tracks);
            let credits = Credits::load(&db.conn, &tracks, &[]);
            let hearts = Hearts::load(&db.conn, &who);
            drop(db);
            let rows = Rows {
                versions: &versions,
                credits: &credits,
                hearts: hearts.as_ref(),
            };
            let first = (page * PAGE) as usize;
            let html: Html = tracks
                .iter()
                .enumerate()
                .map(|(i, t)| Row::new(t, Lead::Number(first + i + 1), &rows, true, true).html())
                .collect();
            Some((html, tracks.is_empty(), total, browse::options(&st)?))
        })
        .await
        .ok_or(Refusal::Unavailable)?;
        let view = TracksPage {
            total: count(total, "track", "tracks"),
            toolbar: Toolbar::of(&b, Kind::Tracks, &options),
            rows,
            empty,
            previous: (page > 0).then(|| b.at_page(page - 1)),
            next: ((u64::from(page + 1) * u64::from(PAGE)) < total).then(|| b.at_page(page + 1)),
        };
        Ok(Body::of("Tracks", &view))
    }
}

//! Hearts: favouriting a track, record or artist from the web UI.
//!
//! A heart is the same favourite the apps make: the toggle goes through the
//! Subsonic API's `star`, so the account's row changes, a track the local
//! account favourites is starred upstream, and the account's apps are told to
//! sync. Every copy of a heart on the page carries `data-fav`, and the answer
//! replaces them all, so a track listed twice lights up twice.

use std::collections::HashSet;

use askama::Template;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use koan_core::db::queries;
use koan_kit_web as kit;

use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::blocking;
use crate::subsonic::EntityKind;
use crate::ui::{UiState, flash_bad, may_edit, nothing, open, paths};

/// What the signed-in account has favourited, for drawing hearts. `None` for
/// an account that may not favourite, which is drawn none.
pub(in crate::ui) struct Hearts {
    tracks: HashSet<i64>,
    albums: HashSet<i64>,
    artists: HashSet<i64>,
}

impl Hearts {
    pub(in crate::ui) fn load(conn: &rusqlite::Connection, user: &AuthUser) -> Option<Self> {
        if !may_edit(user) {
            return None;
        }
        Some(Self {
            tracks: queries::load_favourites(conn, user.user_id).ok()?,
            albums: queries::favourite_album_id_set(conn, user.user_id).ok()?,
            artists: queries::favourite_artist_id_set(conn, user.user_id).ok()?,
        })
    }

    pub(in crate::ui) fn track(&self, id: i64) -> Html {
        Heart::new(Kind::Song, id, self.tracks.contains(&id), false).html()
    }

    pub(in crate::ui) fn track_on(&self, id: i64) -> bool {
        self.tracks.contains(&id)
    }

    /// A record's heart, over the corner of its tile.
    pub(in crate::ui) fn tile(&self, id: i64) -> Html {
        Heart::new(Kind::Album, id, self.albums.contains(&id), true).html()
    }

    pub(in crate::ui) fn album(&self, id: i64) -> Html {
        Heart::new(Kind::Album, id, self.albums.contains(&id), false).html()
    }

    pub(in crate::ui) fn artist(&self, id: i64) -> Html {
        Heart::new(Kind::Artist, id, self.artists.contains(&id), false).html()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::ui) enum Kind {
    Song,
    Album,
    Artist,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Song => "song",
            Kind::Album => "album",
            Kind::Artist => "artist",
        }
    }

    fn entity(self) -> EntityKind {
        match self {
            Kind::Song => EntityKind::Song,
            Kind::Album => EntityKind::Album,
            Kind::Artist => EntityKind::Artist,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Kind::Song => "track",
            Kind::Album => "record",
            Kind::Artist => "artist",
        }
    }
}

/// A heart for one thing, filled while it is a favourite. Pressing it asks
/// for the opposite. `tile` places it over the corner of a record's tile.
#[derive(Template)]
#[template(path = "parts/heart.html")]
struct Heart {
    kind: Kind,
    id: i64,
    on: bool,
    tile: bool,
}

impl Heart {
    fn new(kind: Kind, id: i64, on: bool, tile: bool) -> Self {
        Self { kind, id, on, tile }
    }

    fn button(&self) -> kit::Button<'static> {
        let verb = if self.on { "Unfavourite" } else { "Favourite" };
        kit::Button::quiet(format!("{verb} this {}", self.kind.noun()))
            .icon_only(crate::icon::HEART)
            .toggles(crate::icon::HEART_FILLED, self.on)
            .class("text-muted aria-pressed:text-brand")
            .data("fav", key(self.kind, self.id))
            .post(paths::favourite(self.kind.as_str(), self.id, !self.on))
    }
}

/// What every copy of one thing's heart carries, for the answer to find them
/// all.
fn key(kind: Kind, id: i64) -> String {
    format!("{}-{id}", kind.as_str())
}

/// A favourite's heart beside a track row's ⋯ on a narrow screen, where the
/// row's own heart is in its menu: a mark, not a button. The stylesheet shows
/// it while the row's hidden heart is pressed, so it follows a toggle made
/// from the menu.
#[derive(Template)]
#[template(path = "parts/mark.html")]
pub(in crate::ui) struct Mark;

#[derive(serde::Deserialize)]
pub(in crate::ui) struct On {
    on: u8,
}

/// Favourite or unfavourite one thing, and redraw every heart for it.
pub(in crate::ui) async fn toggle(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path((kind, id)): Path<(Kind, i64)>,
    Query(On { on }): Query<On>,
) -> Response {
    let on = on != 0;
    if !may_edit(&user) {
        return flash_bad("This account cannot favourite.");
    }
    let done = blocking(move || {
        let db = open(&s.pool)?;
        Some(crate::subsonic::favourite(
            &db,
            user.user_id,
            &user.username,
            kind.entity(),
            id,
            on,
        ))
    })
    .await;
    match done {
        Some(Ok(())) => {
            let selector = format!("[data-fav=\"{}\"]", key(kind, id));
            askama_datastar::events([Heart::new(kind, id, on, false)
                .html()
                .patch()
                .selector(selector)
                .into()])
        }
        Some(Err(e)) => {
            log::info!("web UI: favourite {} {id}: {e}", kind.as_str());
            flash_bad("That could not be favourited.")
        }
        None => nothing(),
    }
}

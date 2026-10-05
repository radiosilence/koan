//! Hearts: favouriting a track, record or artist from the web UI.
//!
//! A heart is the same favourite the apps make: the toggle goes through the
//! Subsonic API's `star`, so the account's row changes, a track the local
//! account favourites is starred upstream, and the account's apps are told to
//! sync. Every copy of a heart on the page carries `data-fav`, and the answer
//! replaces them all, so a track listed twice lights up twice.

use std::collections::HashSet;

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use koan_core::auth::Role;
use koan_core::db::queries;

use super::{UiState, events, open, patch};
use crate::auth::AuthUser;
use crate::share::blocking;
use crate::subsonic::EntityKind;

/// What the signed-in account has favourited, for drawing hearts. `None` for
/// an account that may not favourite, which is drawn none.
pub(super) struct Hearts {
    pub tracks: HashSet<i64>,
    pub albums: HashSet<i64>,
    pub artists: HashSet<i64>,
}

impl Hearts {
    pub fn load(conn: &rusqlite::Connection, user: &AuthUser) -> Option<Self> {
        if !user.role.has_permission(Role::User) {
            return None;
        }
        Some(Self {
            tracks: queries::load_favourites(conn, user.user_id).ok()?,
            albums: queries::favourite_album_id_set(conn, user.user_id).ok()?,
            artists: queries::favourite_artist_id_set(conn, user.user_id).ok()?,
        })
    }

    pub fn track(&self, id: i64) -> String {
        heart(Kind::Song, id, self.tracks.contains(&id))
    }

    /// A record's heart, over the corner of its tile. The placing is the
    /// wrapper's, so the button inside can be replaced as any other is.
    pub fn tile(&self, id: i64) -> String {
        format!(
            "<span class=\"absolute top-2 right-2 rounded-full bg-surface/85\">{}</span>",
            heart(Kind::Album, id, self.albums.contains(&id))
        )
    }

    pub fn album(&self, id: i64) -> String {
        heart(Kind::Album, id, self.albums.contains(&id))
    }

    pub fn artist(&self, id: i64) -> String {
        heart(Kind::Artist, id, self.artists.contains(&id))
    }

    /// A favourite's heart beside a track row's ⋯ on a narrow screen, where
    /// the row's own heart is in its menu: a mark, not a button. The stylesheet
    /// shows it while the row's hidden heart is pressed, so it follows a
    /// toggle made from the menu.
    pub fn mark(&self) -> String {
        format!(
            "<span class=\"fav-mark\" role=img aria-label=Favourite title=Favourite>\
<svg viewBox=\"0 0 24 24\" aria-hidden=true><path d=\"{HEART}\"/></svg></span>"
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Kind {
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

const HEART: &str = "M12 20.5s-7.6-4.6-9.5-9.1C1.1 8 3.2 4.5 6.7 4.5c2 0 3.6 1.1 4.5 2.6L12 8.3l.8-1.2c.9-1.5 2.5-2.6 4.5-2.6 3.5 0 5.6 3.5 4.2 6.9-1.9 4.5-9.5 9.1-9.5 9.1z";

/// A heart for one thing, filled while it is a favourite. Pressing it asks
/// for the opposite.
fn heart(kind: Kind, id: i64, on: bool) -> String {
    let (label, next) = if on {
        (format!("Unfavourite this {}", kind.noun()), 0)
    } else {
        (format!("Favourite this {}", kind.noun()), 1)
    };
    format!(
        "<button class=\"quiet text-muted aria-pressed:text-brand\" data-fav=\"{k}-{id}\" aria-pressed=\"{on}\" \
aria-label=\"{label}\" title=\"{label}\" data-on:click=\"@post('/favourite/{k}/{id}?on={next}')\">\
<svg class=\"inline size-[15px] fill-none stroke-current stroke-2 align-[-2px] in-aria-pressed:fill-current\" \
viewBox=\"0 0 24 24\" aria-hidden=true><path d=\"{HEART}\"/></svg></button>",
        k = kind.as_str(),
    )
}

#[derive(serde::Deserialize)]
pub(super) struct On {
    on: u8,
}

/// Favourite or unfavourite one thing, and redraw every heart for it.
pub(super) async fn toggle(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path((kind, id)): Path<(Kind, i64)>,
    Query(On { on }): Query<On>,
) -> Response {
    let on = on != 0;
    let selector = format!("[data-fav=\"{}-{id}\"]", kind.as_str());
    if !user.role.has_permission(Role::User) {
        return events(Vec::new());
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
        Some(Ok(())) => events(vec![patch(
            &heart(kind, id, on),
            Some((&selector, "outer")),
        )]),
        Some(Err(e)) => {
            log::info!("web UI: favourite {} {id}: {e}", kind.as_str());
            events(Vec::new())
        }
        None => events(Vec::new()),
    }
}

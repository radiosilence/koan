//! A playlist, laid out like a record so its rows play and queue the same
//! way: the account's own, or anyone's public one.

use askama::Template;
use axum::extract::Path;
use koan_core::db::queries::{self, PlaylistRow};

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::{blocking, duration};
use crate::ui::parts::count;
use crate::ui::parts::covers::Versions;
use crate::ui::parts::credit::Credits;
use crate::ui::parts::heart::Hearts;
use crate::ui::parts::track::{Lead, Row, Rows};
use crate::ui::{Logged, UiState, open};

pub(in crate::ui) struct Playlist;

#[derive(Template)]
#[template(path = "playlist/page.html")]
struct PlaylistPage<'a> {
    list: &'a PlaylistRow,
    cover: Option<String>,
    sub: String,
    rows: Html,
}

impl PlaylistPage<'_> {
    fn comment(&self) -> Option<&str> {
        self.list.comment.as_deref().filter(|c| !c.is_empty())
    }

    fn size(&self) -> u32 {
        crate::covers::LARGE
    }
}

impl Page for Playlist {
    const SECTION: Section = Section::Playlists;
    type Args = Path<i64>;

    async fn body(s: &UiState, user: &AuthUser, Path(id): Path<i64>) -> Result<Body, Refusal> {
        let (st, who) = (s.clone(), user.clone());
        let (list, cover, n, rows) = blocking(move || {
            let db = open(&st.pool)?;
            let me = queries::auth::resolve_user(&db.conn, who.user_id).logged()?;
            let Some(list) = queries::get_playlist(&db.conn, id)
                .logged()?
                .filter(|p| p.readable_by(me))
            else {
                return Some(Err(Refusal::NotFound));
            };
            let tracks = queries::playlist_tracks(&db.conn, id).logged()?;
            let versions = Versions::of_tracks(&db.conn, &tracks);
            let credits = Credits::load(&db.conn, &tracks, &[]);
            let hearts = Hearts::load(&db.conn, &who);
            let rows = Rows {
                versions: &versions,
                credits: &credits,
                hearts: hearts.as_ref(),
            };
            let html: Html = tracks
                .iter()
                .enumerate()
                .map(|(i, t)| Row::new(t, Lead::Number(i + 1), &rows, true, true).html())
                .collect();
            let cover = tracks
                .iter()
                .find_map(|t| t.album_id)
                .map(|a| versions.url(a, crate::covers::LARGE));
            Some(Ok((list, cover, tracks.len(), html)))
        })
        .await
        .ok_or(Refusal::Unavailable)??;
        let mut sub = Vec::new();
        if let Some(owner) = list.owner.as_deref().filter(|o| !o.is_empty()) {
            sub.push(owner.to_owned());
        }
        sub.push(count(n as u64, "track", "tracks"));
        sub.push(duration(Some(list.duration_ms)));
        let view = PlaylistPage {
            list: &list,
            cover,
            sub: sub.join(" · "),
            rows,
        };
        Ok(Body::of(list.name.clone(), &view))
    }
}

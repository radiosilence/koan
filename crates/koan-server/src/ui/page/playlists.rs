//! Playlists: the account's own and anyone's public ones.

use askama::Template;
use koan_core::db::queries::{self, PlaylistRow};
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::share::{blocking, duration};
use crate::ui::parts::count;
use crate::ui::{Logged, UiState, open, paths};

pub(in crate::ui) struct Playlists;

#[derive(Template)]
#[template(path = "playlists/page.html")]
struct PlaylistsPage {
    lists: Vec<PlaylistRow>,
}

impl PlaylistsPage {
    fn href(&self, p: &PlaylistRow) -> String {
        paths::playlist(p.id)
    }

    fn detail(&self, p: &PlaylistRow) -> String {
        format!(
            "{} · {}",
            count(p.track_count as u64, "track", "tracks"),
            duration(Some(p.duration_ms))
        )
    }
}

impl Page for Playlists {
    const SECTION: Section = Section::Playlists;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        let (st, id) = (s.clone(), user.user_id);
        let lists = blocking(move || queries::list_playlists(&open(&st.pool)?.conn, id).logged())
            .await
            .ok_or(Refusal::Unavailable)?;
        Ok(Body::of("Playlists", &PlaylistsPage { lists }))
    }
}

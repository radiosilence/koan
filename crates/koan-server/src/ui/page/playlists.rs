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

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn playlists_list_the_callers_own_and_play_like_albums() {
        let f = setup(true);
        let (mine, theirs) = {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            let bob = queries::auth::create_user(&db.conn, "bob", "hunter3", Role::User).unwrap();
            let mine = queries::create_playlist(&db.conn, 1, "Damp <Mix>", None).unwrap();
            queries::add_tracks(&db.conn, mine, &[f.track_id]).unwrap();
            let theirs = queries::create_playlist(&db.conn, bob, "Bob's Secret", None).unwrap();
            (mine, theirs)
        };
        let r = send(
            &f.app,
            authed(&f.state, "/playlists").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.contains("Damp &#60;Mix&#62;") && r.body.contains("1 track"));
        assert!(
            !r.body.contains("Bob"),
            "another account's private playlist is not listed"
        );

        let uri = format!("/playlist/{mine}");
        let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.contains("data-act=\"queue\"") && r.body.contains("data-context=\"album\""));
        assert!(r.body.contains(&format!("data-id=\"{}\"", f.track_id)));

        let uri = format!("/playlist/{theirs}");
        let r = send(&f.app, authed(&f.state, &uri).body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
    }
}

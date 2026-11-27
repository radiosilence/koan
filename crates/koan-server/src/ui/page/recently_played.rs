//! Recently played: what the account played lately, each once and newest
//! first by its latest play, as the apps' Recently played shelf.

use koan_core::shelves::Shelf;

use super::{Body, Page, Refusal, Section, favourites::shelf};
use crate::auth::AuthUser;
use crate::ui::UiState;

pub(in crate::ui) struct RecentlyPlayed;

impl Page for RecentlyPlayed {
    const SECTION: Section = Section::RecentlyPlayed;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        shelf(
            s,
            user,
            Shelf::Recent,
            "Recently played",
            "Nothing played in the last 30 days.",
            "What you play here, in the kōan apps or in a Subsonic app signed in as you, is gathered here for a month, each artist, record and track once.",
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn recently_played_is_the_callers_own_each_once() {
        let f = setup(true);
        let now = chrono::Utc::now().timestamp();
        {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            let bob = queries::auth::create_user(&db.conn, "bob", "hunter3", Role::User).unwrap();
            queries::record_play_at(&db.conn, bob, f.track_id, now, None, "local").unwrap();
        }
        let page = || authed(&f.state, "/recent").body(Body::empty()).unwrap();
        let r = send(&f.app, page()).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(
            r.body.contains("Nothing played in the last 30 days"),
            "bob's play is not alice's"
        );

        {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            for ago in [0, 60, 120] {
                queries::record_play_at(&db.conn, 1, f.track_id, now - ago, None, "local").unwrap();
            }
            // Older than the month, and not counted.
            queries::record_play_at(&db.conn, 1, f.track_id, now - 40 * 86_400, None, "local")
                .unwrap();
        }
        let r = send(&f.app, page()).await;
        assert_eq!(
            r.body
                .matches(&format!("data-id=\"{}\"", f.track_id))
                .count(),
            1,
            "three plays, one track"
        );
        assert!(r.body.contains(&format!("href=\"album/{}\"", f.album_id)));
    }
}

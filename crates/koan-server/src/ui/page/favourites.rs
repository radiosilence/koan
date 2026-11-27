//! Favourites: the account's favourite artists, records and tracks, as a
//! shelf.

use askama::Template;
use koan_core::shelves::Shelf;
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::blocking;
use crate::ui::UiState;
use crate::ui::parts::shelf::Read;

pub(in crate::ui) struct Favourites;

#[derive(Template)]
#[template(path = "shelf/page.html")]
pub(in crate::ui) struct ShelfPage {
    pub(in crate::ui) title: &'static str,
    pub(in crate::ui) sections: Option<Html>,
    pub(in crate::ui) empty: &'static str,
    pub(in crate::ui) detail: &'static str,
}

/// A shelf page: its title, then its previews, or what it says when empty.
pub(in crate::ui) async fn shelf(
    s: &UiState,
    user: &AuthUser,
    shelf: Shelf<'static>,
    title: &'static str,
    empty: &'static str,
    detail: &'static str,
) -> Result<Body, Refusal> {
    let (st, who) = (s.clone(), user.clone());
    let sections = blocking(move || {
        let read = Read::of(&st, &who, shelf)?;
        Some((!read.is_empty()).then(|| read.draw(shelf)))
    })
    .await
    .ok_or(Refusal::Unavailable)?;
    let view = ShelfPage {
        title,
        sections,
        empty,
        detail,
    };
    Ok(Body::of(title, &view))
}

impl Page for Favourites {
    const SECTION: Section = Section::Favourites;
    type Args = ();

    async fn body(s: &UiState, user: &AuthUser, (): ()) -> Result<Body, Refusal> {
        shelf(
            s,
            user,
            Shelf::Favourites,
            "Favourites",
            "Nothing favourited yet.",
            "Artists, records and tracks favourited in the kōan apps, or in any Subsonic app signed in as you, are listed here.",
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn favourites_are_the_callers_own_as_a_shelf() {
        let f = setup(true);
        let artist = {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            let bob = queries::auth::create_user(&db.conn, "bob", "hunter3", Role::User).unwrap();
            queries::add_favourite(&db.conn, bob, f.track_id).unwrap();
            queries::set_favourite_album(&db.conn, bob, f.album_id, true).unwrap();
            queries::tracks_by_ids(&db.conn, &[f.track_id]).unwrap()[0]
                .artist_id
                .unwrap()
        };
        let page = || authed(&f.state, "/favourites").body(Body::empty()).unwrap();

        let r = send(&f.app, page()).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(
            r.body.contains("Nothing favourited yet") && !r.body.contains("data-id="),
            "another account's favourites are not alice's"
        );

        {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            queries::add_favourite(&db.conn, 1, f.track_id).unwrap();
            queries::set_favourite_album(&db.conn, 1, f.album_id, true).unwrap();
            queries::set_favourite_artist(&db.conn, 1, artist, true).unwrap();
        }
        let r = send(&f.app, page()).await;
        assert!(
            r.body.contains(&format!("href=\"artist/{artist}\"")),
            "the artist, as a pill"
        );
        assert!(
            r.body.contains(&format!("href=\"album/{}\"", f.album_id)),
            "the record, as a tile"
        );
        assert!(
            r.body.contains(&format!("data-id=\"{}\"", f.track_id))
                && r.body.contains("data-context=\"album\"")
                && r.body.contains("Wet &#60;Moss&#62; &#38; Stone"),
            "the track, in a list that plays on"
        );

        let r = send(
            &f.app,
            authed(&f.state, "/library").body(Body::empty()).unwrap(),
        )
        .await;
        assert!(r.body.contains("href=\"favourites\"") && r.body.contains("href=\"playlists\""));
    }
}

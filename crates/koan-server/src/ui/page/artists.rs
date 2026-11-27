//! Artists: every artist the filters let through, in their order.

use askama::Template;
use askama_datastar::View;
use koan_core::db::queries::{self, ArtistRow};
use koan_core::shelves;
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::blocking;
use crate::ui::parts::browse::{self, Browse, Kind, Toolbar};
use crate::ui::parts::count;
use crate::ui::{Logged, UiState, open, paths};

pub(in crate::ui) struct Artists;

#[derive(Template)]
#[template(path = "artists/page.html")]
struct ArtistsPage {
    artists: Vec<ArtistRow>,
    filtered: bool,
    toolbar: Html,
}

impl ArtistsPage {
    fn href(&self, a: &ArtistRow) -> String {
        paths::artist(a.id)
    }

    fn albums(&self, a: &ArtistRow) -> String {
        count(a.album_count as u64, "album", "albums")
    }
}

impl Page for Artists {
    const SECTION: Section = Section::Artists;
    type Args = View<Browse>;

    async fn body(s: &UiState, user: &AuthUser, View(b): View<Browse>) -> Result<Body, Refusal> {
        let (st, bb, id) = (s.clone(), b.clone(), user.user_id);
        let (artists, options) = blocking(move || {
            let db = open(&st.pool)?;
            let artists =
                queries::list_artists(&db.conn, &bb.artists(id, shelves::now())).logged()?;
            drop(db);
            Some((artists, browse::options(&st)?))
        })
        .await
        .ok_or(Refusal::Unavailable)?;
        let view = ArtistsPage {
            filtered: b.filtered(),
            toolbar: Toolbar::of(&b, Kind::Artists, &options),
            artists,
        };
        Ok(Body::of("Artists", &view))
    }
}

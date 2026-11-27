//! Albums: every record the filters let through, in their order, as tiles.
//! Whole, as the apps list them: the covers load lazily, so the page costs
//! markup and not images.

use askama::Template;
use askama_datastar::View;
use koan_core::db::queries;
use koan_core::shelves;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::blocking;
use crate::ui::parts::browse::{self, Browse, Kind, Toolbar};
use crate::ui::parts::covers::Versions;
use crate::ui::parts::credit::Credits;
use crate::ui::parts::heart::Hearts;
use crate::ui::parts::tile::Tiles;
use crate::ui::{Logged, UiState, open};

pub(in crate::ui) struct Albums;

#[derive(Template)]
#[template(path = "albums/page.html")]
struct AlbumsPage {
    count: usize,
    filtered: bool,
    toolbar: Html,
    tiles: Html,
}

impl Page for Albums {
    const SECTION: Section = Section::Albums;
    type Args = View<Browse>;

    async fn body(s: &UiState, user: &AuthUser, View(b): View<Browse>) -> Result<Body, Refusal> {
        let b = b.seeded();
        let (st, bb, who) = (s.clone(), b.clone(), user.clone());
        let (count, tiles, options) = blocking(move || {
            let db = open(&st.pool)?;
            let albums = queries::list_albums(&db.conn, &bb.albums(who.user_id, shelves::now())).logged()?;
            let versions = Versions::of_albums(&db.conn, &albums);
            let ids: Vec<i64> = albums.iter().map(|a| a.id).collect();
            let credits = Credits::load(&db.conn, &[], &ids);
            let hearts = Hearts::load(&db.conn, &who);
            drop(db);
            let tiles = Tiles::of(&albums, &versions, &credits, hearts.as_ref());
            Some((albums.len(), tiles, browse::options(&st)?))
        })
        .await
        .ok_or(Refusal::Unavailable)?;
        let view = AlbumsPage {
            count,
            filtered: b.filtered(),
            toolbar: Toolbar::of(&b, Kind::Albums, &options),
            tiles,
        };
        Ok(Body::of("Albums", &view))
    }
}

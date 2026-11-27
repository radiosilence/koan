//! An artist: their own records, then the ones they feature on, to favourite
//! and share.

use askama::Template;
use axum::Extension;
use axum::extract::{Path, State};
use axum::response::Response;
use koan_core::db::queries::{self, ArtistRow};
use koan_core::helpers::ShareTarget;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::blocking;
use crate::ui::parts::count;
use crate::ui::parts::covers::Versions;
use crate::ui::parts::credit::Credits;
use crate::ui::parts::heart::Hearts;
use crate::ui::parts::share::{self, Shared};
use crate::ui::parts::tile::Tiles;
use crate::ui::{Logged, UiState, open, paths};

pub(in crate::ui) struct Artist;

/// `heart` and `share` are there where the account may favourite, and so
/// share.
#[derive(Template)]
#[template(path = "artist/page.html")]
struct ArtistPage<'a> {
    artist: &'a ArtistRow,
    heart: Option<Html>,
    share: Option<Html>,
    tiles: Option<Html>,
    features: Option<Html>,
}

impl ArtistPage<'_> {
    fn sub(&self) -> String {
        format!(
            "{} · {}",
            count(self.artist.album_count as u64, "album", "albums"),
            count(self.artist.track_count as u64, "track", "tracks")
        )
    }

    fn share_button(&self) -> koan_kit_web::Button<'static> {
        koan_kit_web::Button::standard("Share").post(paths::artist_share(self.artist.id))
    }
}

impl Page for Artist {
    const SECTION: Section = Section::Artists;
    type Args = Path<i64>;

    async fn body(s: &UiState, user: &AuthUser, Path(id): Path<i64>) -> Result<Body, Refusal> {
        let (st, who) = (s.clone(), user.clone());
        let (artist, heart, tiles, features) = blocking(move || {
            let db = open(&st.pool)?;
            let Some(artist) = queries::get_artist(&db.conn, id).logged()? else {
                return Some(Err(Refusal::NotFound));
            };
            let albums = queries::albums_for_artist(&db.conn, id).logged()?;
            let features = queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    features_of: Some(id),
                    order: queries::AlbumOrder::Date,
                    ..Default::default()
                },
            )
            .unwrap_or_default();
            let ids: Vec<i64> = albums.iter().chain(&features).map(|a| a.id).collect();
            let versions = Versions::of(&db.conn, &ids);
            let credits = Credits::load(&db.conn, &[], &ids);
            let hearts = Hearts::load(&db.conn, &who);
            let draw = |list: &[queries::AlbumRow]| {
                (!list.is_empty()).then(|| Tiles::of(list, &versions, &credits, hearts.as_ref()))
            };
            Some(Ok((
                artist.clone(),
                hearts.as_ref().map(|h| h.artist(artist.id)),
                draw(&albums),
                draw(&features),
            )))
        })
        .await
        .ok_or(Refusal::Unavailable)??;
        let view = ArtistPage {
            artist: &artist,
            share: heart.is_some().then(Shared::empty),
            heart,
            tiles,
            features,
        };
        Ok(Body::of(artist.name.clone(), &view))
    }
}

pub(in crate::ui) async fn share(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    share::make(s, user, ShareTarget::Artist(id)).await
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

    #[tokio::test]
    async fn an_artist_page_lists_the_records_they_feature_on() {
        let f = setup(true);
        let artist = {
            let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
            let file = f.dir.path().join("guest.flac");
            std::fs::write(&file, b"0123456789").unwrap();
            let mut m = meta(&file, "Guest Spot", 1);
            m.album = "Other People".into();
            m.album_artist = Some("Bob Ostertag".into());
            queries::upsert_track(&db.conn, &m).unwrap();
            queries::tracks_by_ids(&db.conn, &[f.track_id]).unwrap()[0]
                .artist_id
                .unwrap()
        };
        let req = authed(&f.state, &format!("/artist/{artist}"))
            .body(Body::empty())
            .unwrap();
        let html = send(&f.app, req).await.body;
        let (own, features) = html
            .split_once(">Features on</h2>")
            .unwrap_or_else(|| panic!("a Features on section: {html}"));
        assert!(own.contains("Hymn &#60;to&#62; Moisture"), "{html}");
        assert!(!own.contains("Other People"), "{html}");
        assert!(features.contains("Other People"), "{html}");
    }
}

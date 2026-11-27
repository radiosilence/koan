//! A record: its cover, credit and tracks, disc by disc, to play, queue,
//! favourite and share.

use askama::Template;
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use koan_core::db::queries::{self, AlbumRow};
use koan_core::helpers::ShareTarget;
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::{Html, Render};
use crate::share::{blocking, duration};
use crate::ui::parts::covers::Versions;
use crate::ui::parts::credit::{Credit, Credits};
use crate::ui::parts::heart::Hearts;
use crate::ui::parts::share::{self, Shared};
use crate::ui::parts::track::{Lead, Row, Rows};
use crate::ui::parts::{count, year};
use crate::ui::{Logged, UiState, may_edit, open, paths};

pub(in crate::ui) struct Album;

#[derive(Template)]
#[template(path = "album/page.html")]
struct AlbumPage<'a> {
    album: &'a AlbumRow,
    cover: String,
    credit: Html,
    /// What follows the credit under the title.
    sub: Vec<String>,
    /// The share button and its region, where the account may share.
    share: Option<Html>,
    heart: Html,
    /// Each row, after its disc's heading where the album has several discs.
    rows: Vec<(Option<i32>, Html)>,
}

impl AlbumPage<'_> {
    fn share_button(&self) -> koan_kit_web::Button<'static> {
        koan_kit_web::Button::standard("Share").post(paths::album_share(self.album.id, None))
    }

    fn size(&self) -> u32 {
        crate::covers::LARGE
    }
}

impl Page for Album {
    const SECTION: Section = Section::Albums;
    type Args = Path<i64>;

    async fn body(s: &UiState, user: &AuthUser, Path(id): Path<i64>) -> Result<Body, Refusal> {
        let (st, who) = (s.clone(), user.clone());
        let ((album, credit, heart, n, total), rows, versions) = blocking(move || {
            let db = open(&st.pool)?;
            let Some(album) = queries::get_album(&db.conn, id).logged()? else {
                return Some(Err(Refusal::NotFound));
            };
            let tracks = queries::tracks_for_album(&db.conn, id).logged()?;
            let versions = Versions::of(&db.conn, &[id]);
            let credits = Credits::load(&db.conn, &tracks, &[id]);
            let hearts = Hearts::load(&db.conn, &who);
            let share = may_edit(&who);
            let rows = Rows {
                versions: &versions,
                credits: &credits,
                hearts: hearts.as_ref(),
            };
            let discs = tracks
                .iter()
                .map(|t| t.disc.unwrap_or(1))
                .collect::<std::collections::BTreeSet<_>>();
            let mut disc = None;
            let drawn: Vec<(Option<i32>, Html)> = tracks
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let d = t.disc.unwrap_or(1);
                    let heading = (discs.len() > 1 && disc != Some(d)).then_some(d);
                    disc = Some(d);
                    let n = t.track_number.map_or(i + 1, |n| n as usize);
                    let row = Row::new(t, Lead::Number(n), &rows, credits.row(t).differ(), false);
                    (heading, if share { row.shared() } else { row }.html())
                })
                .collect();
            let total: i64 = tracks.iter().filter_map(|t| t.duration_ms).sum();
            let credit = Credit::links(&credits.album(
                Some(album.id),
                &album.artist_name,
                Some(album.artist_id),
            ));
            let heart = hearts
                .as_ref()
                .map(|h| h.album(album.id))
                .unwrap_or_default();
            Some(Ok((
                (album, credit, heart, tracks.len(), total),
                drawn,
                versions,
            )))
        })
        .await
        .ok_or(Refusal::Unavailable)??;
        let mut sub = Vec::new();
        let y = year(album.date.as_deref());
        if !y.is_empty() {
            sub.push(y.to_owned());
        }
        sub.push(count(n as u64, "track", "tracks"));
        sub.push(duration(Some(total)));
        if let Some(codec) = &album.codec {
            sub.push(codec.clone());
        }
        let view = AlbumPage {
            album: &album,
            cover: versions.url(album.id, crate::covers::LARGE),
            credit,
            sub,
            share: may_edit(user).then(Shared::empty),
            heart,
            rows,
        };
        Ok(Body::of(album.title.clone(), &view))
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(in crate::ui) struct Cue {
    track: Option<i64>,
}

/// Share the album, or with `?track=` the album cued to that track.
pub(in crate::ui) async fn share(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Query(cue): Query<Cue>,
) -> Response {
    let target = ShareTarget::Album {
        album_id: id,
        start_track_id: cue.track,
    };
    share::make(s, user, target).await
}

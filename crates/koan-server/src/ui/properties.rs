//! A track's properties: which file on the server it is, what each of its
//! sources says, and the ids that name it. Read-only, for finding out why a
//! record turns up twice or the wrong file plays.
//!
//! Paths describe the server's filesystem, so only an admin is shown them.
//!
//! A page like any other; the UI's script shows its content in the shell's
//! dialog.

use askama::Template;
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use koan_core::auth::Role;
use koan_core::db::queries::{self, UidKind};

use super::pages::{Credits, credit_links, link, respond, unavailable};
use super::{UiState, open};
use crate::auth::AuthUser;
use crate::share::{blocking, duration, not_found};

pub(super) const LIST: &str = "m-0 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1 text-control \
[&_dd]:m-0 [&_dd]:wrap-anywhere [&_dd]:select-text [&_dt]:text-muted";
pub(super) const SECTION: &str = "mt-5 mb-2 text-meta";

/// One row of a definition list. `html` values are the server's own markup:
/// links to the artist and album pages.
struct Field {
    name: &'static str,
    value: String,
    html: bool,
}

#[derive(Template)]
#[template(path = "properties/page.html")]
struct Properties<'a> {
    title: &'a str,
    /// The artist credit, as links.
    artist: &'a str,
    /// The file on the server, for an admin.
    path: Option<&'a str>,
    /// Each list under its heading: the track's, then each source's.
    lists: Vec<(&'static str, Vec<Field>)>,
}

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let admin = user.role == Role::Admin;
    let state = s.clone();
    match blocking(move || render(&state, id, admin)).await {
        Some(Some(body)) => respond(&s, &headers, &user, "Properties", &body),
        Some(None) => not_found(),
        None => unavailable(),
    }
}

/// `None` when the library cannot be read, `Some(None)` for a track it does
/// not have.
fn render(s: &UiState, id: i64, admin: bool) -> Option<Option<String>> {
    let db = open(&s.pool)?;
    let conn = &db.conn;
    let Some(t) = queries::tracks_by_ids(conn, &[id]).ok()?.pop() else {
        return Some(None);
    };
    let album = t
        .album_id
        .map(|a| queries::get_album(conn, a))
        .transpose()
        .ok()?
        .flatten();
    let uid = |kind, id: i64| {
        queries::uids_for(conn, kind, [id])
            .ok()
            .map(|mut m| m.remove(&id))
    };
    let track_uid = uid(UidKind::Track, t.id)?;
    let album_uid = match t.album_id {
        Some(a) => uid(UidKind::Album, a)?,
        None => None,
    };
    let sources = queries::sources_of_track(conn, id).ok()?;
    let credits = Credits::load(conn, std::slice::from_ref(&t), t.album_id.as_slice());
    let artist = credit_links(&credits.track(&t), false);
    let album_artist = credit_links(
        &credits.album(
            t.album_id,
            &t.album_artist_name,
            album.as_ref().map(|a| a.artist_id),
        ),
        false,
    );
    // Facts only a source holds, from the first that has them: the file's.
    let first = |f: fn(&queries::TrackMeta) -> Option<String>| sources.iter().find_map(f);
    let size = first(|m| m.size_bytes.map(|b| format!("{b} bytes")));
    let date = album
        .as_ref()
        .and_then(|a| a.date.clone())
        .or_else(|| first(|m| m.date.clone()));

    let path = if admin {
        crate::subsonic::track_file_path(&t)
    } else {
        None
    };
    let number = |v: Option<i32>| v.map(|v| v.to_string());
    let text = |name, value: Option<String>| (name, value, false);
    let html = |name, value: Option<String>| (name, value, true);
    // Names that have a page are linked to it.
    let linked = |kind, id, name: &str| (!name.is_empty()).then(|| link(kind, id, name));
    let track = [
        text("Title", Some(t.title.clone())),
        html("Artist", Some(artist.clone())),
        html("Album artist", Some(album_artist)),
        html("Album", linked("album", t.album_id, &t.album_title)),
        text("Disc", number(t.disc)),
        text("Track", number(t.track_number)),
        text("Discs", album.as_ref().and_then(|a| number(a.total_discs))),
        text("Date", date),
        text("Genre", t.genre.clone()),
        text("Codec", t.codec.clone()),
        text("Sample rate", t.sample_rate.map(|r| format!("{r} Hz"))),
        text("Bit depth", t.bit_depth.map(|b| format!("{b} bit"))),
        text("Length", Some(duration(t.duration_ms))),
        text("Size", size),
        text("Track uid", track_uid),
        text("Album uid", album_uid),
        text("MusicBrainz recording", first(|m| m.mbid.clone())),
        text("MusicBrainz release", first(|m| m.album_mbid.clone())),
    ];
    let mut lists = vec![(
        "Track",
        track
            .into_iter()
            .filter_map(|(name, value, html)| {
                let value = value.filter(|v| !v.is_empty())?;
                Some(Field { name, value, html })
            })
            .collect(),
    )];
    for mut source in sources {
        if !admin {
            source.path = None;
            source.server_path = None;
        }
        let heading = if source.source == "remote" {
            "From the remote server"
        } else {
            "From the file"
        };
        let fields = source
            .info_fields()
            .into_iter()
            .map(|(name, value)| Field {
                name,
                value,
                html: false,
            })
            .collect();
        lists.push((heading, fields));
    }
    let page = Properties {
        title: &t.title,
        artist: &artist,
        path,
        lists,
    };
    Some(Some(page.render().unwrap_or_default()))
}

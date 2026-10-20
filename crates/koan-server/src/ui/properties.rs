//! A track's properties: which file on the server it is, what each of its
//! sources says, and the ids that name it. Read-only, for finding out why a
//! record turns up twice or the wrong file plays.
//!
//! Paths describe the server's filesystem, so only an admin is shown them.
//!
//! A page like any other; the UI's script shows its content in the shell's
//! dialog.

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use koan_core::auth::Role;
use koan_core::db::queries::{self, UidKind};

use super::pages::{COPY_INPUT, Credits, KICKER, SUB, credit_links, link, respond, unavailable};
use super::{UiState, open};
use crate::auth::AuthUser;
use crate::share::{blocking, duration, escape, not_found};

const LIST: &str = "m-0 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1 text-control \
[&_dd]:m-0 [&_dd]:wrap-anywhere [&_dd]:select-text [&_dt]:text-muted";
const SECTION: &str = "mt-5 mb-2 text-meta";

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

    let mut out = format!(
        "<p class=\"{KICKER}\">Properties</p><h2 class=\"my-1 normal-case\">{}</h2><p class=\"{SUB}\">{}</p>",
        escape(&t.title),
        artist,
    );
    if admin && let Some(path) = crate::subsonic::track_file_path(&t) {
        out.push_str(&format!(
            "<p class=\"{SECTION}\">File on the server</p><div class=\"flex gap-2\">\
<input id=track-path readonly value=\"{path}\" aria-label=\"File on the server\" class=\"{COPY_INPUT}\">\
<button data-copy=track-path>Copy</button></div>",
            path = escape(path)
        ));
    }

    let number = |v: Option<i32>| v.map(|v| v.to_string());
    let text = |v: Option<String>| v.map(|v| escape(&v));
    // Names that have a page are linked to it.
    let linked = |kind, id, name: &str| (!name.is_empty()).then(|| link(kind, id, name));
    let track = [
        ("Title", text(Some(t.title.clone()))),
        ("Artist", Some(artist)),
        ("Album artist", Some(album_artist)),
        ("Album", linked("album", t.album_id, &t.album_title)),
        ("Disc", number(t.disc)),
        ("Track", number(t.track_number)),
        ("Discs", album.as_ref().and_then(|a| number(a.total_discs))),
        ("Date", text(date)),
        ("Genre", text(t.genre.clone())),
        ("Codec", text(t.codec.clone())),
        ("Sample rate", t.sample_rate.map(|r| format!("{r} Hz"))),
        ("Bit depth", t.bit_depth.map(|b| format!("{b} bit"))),
        ("Length", Some(duration(t.duration_ms))),
        ("Size", size),
        ("Track uid", text(track_uid)),
        ("Album uid", text(album_uid)),
        ("MusicBrainz recording", text(first(|m| m.mbid.clone()))),
        ("MusicBrainz release", text(first(|m| m.album_mbid.clone()))),
    ];
    out.push_str(&format!("<p class=\"{SECTION}\">Track</p>"));
    out.push_str(&list(track.into_iter().filter_map(|(name, value)| {
        Some((name, value.filter(|v| !v.is_empty())?))
    })));

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
        out.push_str(&format!("<p class=\"{SECTION}\">{heading}</p>"));
        out.push_str(&list(
            source
                .info_fields()
                .into_iter()
                .map(|(name, value)| (name, escape(&value))),
        ));
    }
    Some(Some(out))
}

/// A definition list of `fields`, whose values are HTML.
fn list(fields: impl Iterator<Item = (&'static str, String)>) -> String {
    let rows: String = fields
        .map(|(name, value)| format!("<dt>{name}</dt><dd>{value}</dd>"))
        .collect();
    format!("<dl class=\"{LIST}\">{rows}</dl>")
}

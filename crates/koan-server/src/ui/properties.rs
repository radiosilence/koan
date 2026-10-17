//! A track's properties: which file on the server it is, what each of its
//! sources says, and the ids that name it. Read-only, for finding out why a
//! record turns up twice or the wrong file plays.
//!
//! Paths describe the server's filesystem, so only an admin is shown them.

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use koan_core::auth::Role;
use koan_core::db::queries::{self, UidKind};

use super::pages::{COPY_INPUT, COPY_ROW, KICKER, SUB};
use super::{UiState, html, open};
use crate::auth::AuthUser;
use crate::share::{blocking, duration, escape, not_found};

/// The labels of fields that name a place on the server's disk.
const PATH_FIELDS: &[&str] = &["Path"];

const LIST: &str = "m-0 grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1 text-control \
[&_dd]:m-0 [&_dd]:wrap-anywhere [&_dd]:select-text [&_dt]:text-muted";
const SECTION: &str = "mt-5 mb-2 text-meta";

pub(super) async fn page(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    let admin = user.role == Role::Admin;
    match blocking(move || render(&s, id, admin)).await {
        Some(body) => html(StatusCode::OK, body),
        None => not_found(),
    }
}

fn render(s: &UiState, id: i64, admin: bool) -> Option<String> {
    let db = open(&s.pool)?;
    let conn = &db.conn;
    let t = queries::tracks_by_ids(conn, &[id]).ok()?.pop()?;
    let album = t
        .album_id
        .and_then(|a| queries::get_album(conn, a).ok().flatten());
    let uid = |kind, id: Option<i64>| {
        let id = id?;
        queries::uids_for(conn, kind, [id]).ok()?.remove(&id)
    };
    let (track_uid, album_uid) = (
        uid(UidKind::Track, Some(t.id)),
        uid(UidKind::Album, t.album_id),
    );
    let sources = queries::sources_of_track(conn, id).ok()?;
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
        escape(&t.artist_name),
    );
    if admin && let Some(path) = crate::subsonic::track_file_path(&t) {
        out.push_str(&format!(
            "<p class=\"{SECTION}\">File on the server</p><div class=\"{COPY_ROW} mt-0 max-w-none\">\
<input id=track-path readonly value=\"{path}\" aria-label=\"File on the server\" class=\"{COPY_INPUT}\">\
<button data-copy=track-path>Copy</button></div>",
            path = escape(path)
        ));
    }

    let number = |v: Option<i32>| v.map(|v| v.to_string());
    let track = [
        ("Title", Some(t.title.clone())),
        ("Artist", Some(t.artist_name.clone())),
        ("Album artist", Some(t.album_artist_name.clone())),
        ("Album", Some(t.album_title.clone())),
        ("Disc", number(t.disc)),
        ("Track", number(t.track_number)),
        ("Discs", album.as_ref().and_then(|a| number(a.total_discs))),
        ("Date", date),
        ("Genre", t.genre.clone()),
        ("Codec", t.codec.clone()),
        ("Sample rate", t.sample_rate.map(|r| format!("{r} Hz"))),
        ("Bit depth", t.bit_depth.map(|b| format!("{b} bit"))),
        ("Length", t.duration_ms.map(|ms| duration(Some(ms)))),
        ("Size", size),
        ("Track uid", track_uid),
        ("Album uid", album_uid),
        ("MusicBrainz recording", first(|m| m.mbid.clone())),
        ("MusicBrainz release", first(|m| m.album_mbid.clone())),
    ];
    out.push_str(&format!("<p class=\"{SECTION}\">Track</p>"));
    out.push_str(&list(track.into_iter().filter_map(|(name, value)| {
        Some((name, value.filter(|v| !v.is_empty())?))
    })));

    for source in sources {
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
                .filter(|(name, _)| admin || !PATH_FIELDS.contains(name)),
        ));
    }
    Some(format!(
        "{out}<form method=dialog class=\"mt-6 flex justify-end\"><button>Close</button></form>"
    ))
}

fn list(fields: impl Iterator<Item = (&'static str, String)>) -> String {
    let rows: String = fields
        .map(|(name, value)| format!("<dt>{name}</dt><dd>{}</dd>", escape(&value)))
        .collect();
    format!("<dl class=\"{LIST}\">{rows}</dl>")
}

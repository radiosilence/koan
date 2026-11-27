//! A track's properties: which file on the server it is, what each of its
//! sources says, and the ids that name it. Read-only, for finding out why a
//! record turns up twice or the wrong file plays.
//!
//! Paths describe the server's filesystem, so only an admin is shown them.
//!
//! A page like any other, reached from a track row's menu.

use askama::Template;
use axum::extract::Path;
use koan_core::db::queries::{self, UidKind};
use koan_kit_web as kit;

use super::{Body, Page, Refusal, Section};
use crate::auth::AuthUser;
use crate::html::Html;
use crate::share::{blocking, duration};
use crate::ui::parts::credit::{Credit, Credits, Link};
use crate::ui::{Logged, UiState, is_admin, open};

pub(in crate::ui) struct Properties;

#[derive(Template)]
#[template(path = "properties/page.html")]
struct PropertiesPage<'a> {
    title: &'a str,
    artist: &'a Html,
    path: Option<&'a str>,
    lists: Vec<(&'static str, kit::Pairs<'static>)>,
}

impl Page for Properties {
    const SECTION: Section = Section::Tracks;
    type Args = Path<i64>;

    async fn body(s: &UiState, user: &AuthUser, Path(id): Path<i64>) -> Result<Body, Refusal> {
        let admin = is_admin(user);
        let state = s.clone();
        blocking(move || read(&state, id, admin))
            .await
            .ok_or(Refusal::Unavailable)?
    }
}

/// A field's value: text, or names linked to their pages.
enum Value {
    Text(String),
    Links(Html),
}

fn read(s: &UiState, id: i64, admin: bool) -> Option<Result<Body, Refusal>> {
    let db = open(&s.pool)?;
    let conn = &db.conn;
    let Some(t) = queries::tracks_by_ids(conn, &[id]).logged()?.pop() else {
        return Some(Err(Refusal::NotFound));
    };
    let album = t
        .album_id
        .map(|a| queries::get_album(conn, a))
        .transpose()
        .logged()?
        .flatten();
    let uid = |kind, id: i64| {
        queries::uids_for(conn, kind, [id])
            .logged()
            .map(|mut m| m.remove(&id))
    };
    let track_uid = uid(UidKind::Track, t.id)?;
    let album_uid = match t.album_id {
        Some(a) => uid(UidKind::Album, a)?,
        None => None,
    };
    let sources = queries::sources_of_track(conn, id).logged()?;
    let credits = Credits::load(conn, std::slice::from_ref(&t), t.album_id.as_slice());
    let artist = Credit::links(&credits.track(&t));
    let album_artist = Credit::links(&credits.album(
        t.album_id,
        &t.album_artist_name,
        album.as_ref().map(|a| a.artist_id),
    ));
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
    let text = |name, value: Option<String>| (name, value.map(Value::Text));
    let html = |name, value: Option<Html>| (name, value.map(Value::Links));
    let track = [
        text("Title", Some(t.title.clone())),
        html("Artist", Some(artist.clone())),
        html("Album artist", Some(album_artist)),
        html(
            "Album",
            (!t.album_title.is_empty()).then(|| Link::album(t.album_id, &t.album_title)),
        ),
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
    let mut pairs = kit::Pairs::new();
    for (name, value) in track {
        pairs = match value {
            Some(Value::Text(t)) if !t.is_empty() => pairs.pair(name, t),
            Some(Value::Links(h)) if !h.is_empty() => {
                match kit::Markup::render(&Linked { html: h }) {
                    Ok(m) => pairs.pair(name, m),
                    Err(_) => pairs,
                }
            }
            _ => pairs,
        };
    }
    let mut lists = vec![("Track", pairs)];
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
        let mut pairs = kit::Pairs::new();
        for (name, value) in source.info_fields() {
            pairs = pairs.pair(name, value);
        }
        lists.push((heading, pairs));
    }
    let page = PropertiesPage {
        title: &t.title,
        artist: &artist,
        path: path.as_deref(),
        lists,
    };
    Some(Ok(Body::of(format!("{} · Properties", t.title), &page)))
}

/// Linked names as a value of the list.
#[derive(Template)]
#[template(source = "{{ html }}", ext = "html")]
struct Linked {
    html: Html,
}

#[cfg(test)]
mod tests {
    use crate::ui::fixture::*;

#[tokio::test]
async fn track_properties_show_the_file_path_to_admins_only() {
    let f = setup(true);
    let db = Database::open(f.state.pool.path()).unwrap();
    let boss = queries::auth::create_user(&db.conn, "boss", "sesame", Role::Admin).unwrap();
    let admin =
        auth::mint_access_token(&f.state.private_pem, boss, "boss", Role::Admin, 900).unwrap();
    let uri = format!("/track/{}/properties", f.track_id);
    let path = f.dir.path().join("a.flac").to_string_lossy().into_owned();

    let r = send(
        &f.app,
        get(&uri)
            .header("x-koan-partial", "1")
            .header(header::COOKIE, format!("koan_access={admin}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains(&path), "{}", r.body);
    assert!(r.body.contains("Wet &#60;Moss&#62; &#38; Stone"));

    let r = send(
        &f.app,
        authed(&f.state, &uri)
            .header("x-koan-partial", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains(&path), "{}", r.body);
    assert!(!r.body.contains("a.flac"));
    assert!(r.body.contains("Wet &#60;Moss&#62; &#38; Stone"));
    assert!(r.body.contains("FLAC"));

    let r = send(
        &f.app,
        authed(&f.state, "/track/999999/properties")
            .header("x-koan-partial", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}
}

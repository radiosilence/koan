//! The UI's pages, rendered whole for a page load and as content alone for the
//! UI's own navigation, which keeps the shell (and the player in it) in place.
//!
//! Track rows carry what the player needs in data attributes; the player reads
//! them from the page rather than asking the server again.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use koan_core::auth::Role;
use koan_core::db::queries::{self, AlbumOrder, AlbumQuery, AlbumRow, ArtistQuery, TrackRow};
use koan_core::helpers::ShareTarget;

use super::browse::{self, Browse};
use super::{PARTIAL, UiState, events, html, open, patch};
use crate::auth::AuthUser;
use crate::share::{blocking, duration, escape, not_found};

const ALBUMS_PAGE: u32 = 60;
const ARTISTS_PAGE: u32 = 100;
const GENRES_OFFERED: u32 = 80;

const ICON_PREV: &str =
    "<svg viewBox=\"0 0 24 24\" aria-hidden=true><path d=\"M6 5h2v14H6zM20 5v14L9 12z\"/></svg>";
const ICON_NEXT: &str =
    "<svg viewBox=\"0 0 24 24\" aria-hidden=true><path d=\"M16 5h2v14h-2zM4 5v14l11-7z\"/></svg>";
const ICON_PLAY: &str =
    "<svg class=i-play viewBox=\"0 0 24 24\" aria-hidden=true><path d=\"M7 4v16l13-8z\"/></svg>";
const ICON_PAUSE: &str = "<svg class=i-pause viewBox=\"0 0 24 24\" aria-hidden=true>\
     <path d=\"M6 4h4v16H6zM14 4h4v16h-4z\"/></svg>";

fn buttons() -> String {
    format!(
        "<div class=buttons><button class=\"icon quiet\" data-ctl=prev aria-label=Previous>{ICON_PREV}</button>\
<button class=\"icon primary\" data-ctl=play aria-label=\"Play or pause\">{ICON_PLAY}{ICON_PAUSE}</button>\
<button class=\"icon quiet\" data-ctl=next aria-label=Next>{ICON_NEXT}</button></div>"
    )
}

const SCRUB: &str = "<div class=scrub><span data-np=pos>0:00</span>\
<input type=range data-ctl=seek min=0 max=0 step=0.1 value=0 aria-label=Position>\
<span data-np=len>0:00</span></div>";

fn head(title: &str) -> String {
    format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8>\
<meta name=viewport content=\"width=device-width,initial-scale=1,viewport-fit=cover\">\
<meta name=theme-color content=\"#181b1f\"><meta name=robots content=\"noindex,nofollow\">\
<title>{} · kōan</title>{}<link rel=stylesheet href=\"/ui/assets/ui.css\">",
        escape(title),
        crate::share::icon_links("/ui/assets")
    )
}

fn shell(title: &str, content: &str, user: &AuthUser, auth_enabled: bool) -> String {
    let signout = if auth_enabled {
        let users = if user.role == Role::Admin {
            "<a href=\"/users\" data-nav=users>Users</a>"
        } else {
            ""
        };
        format!(
            "<form class=account method=post action=\"/auth/signout\"><span>{}</span>\
{users}<a href=\"/keys\" data-nav=keys>API keys</a><button class=quiet>Sign out</button></form>",
            escape(&user.username)
        )
    } else {
        String::new()
    };
    let account = format!(
        "<div class=side-foot>{signout}<a class=version \
href=\"https://github.com/radiosilence/koan/releases/tag/v{v}\">kōan {v}</a></div>",
        v = env!("CARGO_PKG_VERSION")
    );
    format!(
        "{head}<script type=module src=\"/ui/assets/datastar.js\"></script>\
<script src=\"/ui/assets/player.js\" defer></script><script src=\"/ui/assets/ui.js\" defer></script>\
</head><body><nav class=side aria-label=Library><a class=brand href=\"/\">kōan</a>\
<a href=\"/albums\" data-nav=albums>Albums</a><a href=\"/artists\" data-nav=artists>Artists</a>\
<a href=\"/playlists\" data-nav=playlists>Playlists</a>\
<a href=\"/search\" data-nav=search>Search</a><a href=\"/queue\" data-nav=queue>Queue</a>{account}</nav>\
<main id=content>{content}</main><div class=account-foot>{account}</div>\
<footer class=bar><progress class=progress data-np=progress max=1 value=0></progress>\
<a class=now href=\"/queue\"><img class=thumb data-np=cover alt=\"\" hidden>\
<span class=np><span class=np-title data-np=title>Nothing playing</span>\
<span class=np-artist data-np=artist></span></span></a>\
<div class=transport>{buttons}{SCRUB}</div></footer></body></html>",
        head = head(title),
        buttons = buttons(),
    )
}

/// The whole page, or only its content when the UI's script asked for that.
pub(super) fn respond(
    s: &UiState,
    headers: &HeaderMap,
    user: &AuthUser,
    title: &str,
    class: &str,
    inner: &str,
) -> Response {
    let content = format!(
        "<section class=\"page {class}\" data-title=\"{}\">{inner}</section>",
        escape(title)
    );
    if headers.contains_key(PARTIAL) {
        html(StatusCode::OK, content)
    } else {
        html(StatusCode::OK, shell(title, &content, user, s.auth_enabled))
    }
}

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "the library is unavailable",
    )
        .into_response()
}

pub(super) fn login(next: &str, error: Option<&str>) -> String {
    let error = error
        .map(|e| format!("<p class=error role=alert>{}</p>", escape(e)))
        .unwrap_or_default();
    format!(
        "{head}</head><body class=signin><main><h1>kōan</h1>\
<form method=post action=\"/login\"><input type=hidden name=next value=\"{next}\">\
<label>Username<input name=username autocomplete=username autocapitalize=none spellcheck=false required autofocus></label>\
<label>Password<input name=password type=password autocomplete=current-password required></label>\
{error}<button class=primary>Sign in</button></form></main></body></html>",
        head = head("Sign in"),
        next = escape(next),
    )
}

fn year(date: Option<&str>) -> &str {
    date.and_then(|d| d.get(..4)).unwrap_or("")
}

/// Each album's cover version: when its files last changed. A cover URL
/// carries it, so the URL changes whenever the art might have and the browser
/// can keep each one for good. One query for a page of albums.
type Versions = HashMap<i64, i64>;

fn cover_versions(conn: &rusqlite::Connection, album_ids: &[i64]) -> Versions {
    if album_ids.is_empty() {
        return Versions::new();
    }
    let sql = format!(
        "SELECT album_id, MAX(COALESCE(mtime, 0)) FROM tracks WHERE album_id IN ({}) GROUP BY album_id",
        vec!["?"; album_ids.len()].join(",")
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Versions::new();
    };
    stmt.query_map(rusqlite::params_from_iter(album_ids), |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

fn cover_url(album_id: i64, size: u32, versions: &Versions) -> String {
    format!(
        "/ui/cover/{album_id}?size={size}&v={}",
        versions.get(&album_id).copied().unwrap_or(0)
    )
}

fn album_versions(conn: &rusqlite::Connection, albums: &[AlbumRow]) -> Versions {
    cover_versions(conn, &albums.iter().map(|a| a.id).collect::<Vec<_>>())
}

fn track_versions(conn: &rusqlite::Connection, tracks: &[TrackRow]) -> Versions {
    let mut ids: Vec<i64> = tracks.iter().filter_map(|t| t.album_id).collect();
    ids.sort_unstable();
    ids.dedup();
    cover_versions(conn, &ids)
}

fn cells(albums: &[AlbumRow], versions: &Versions) -> String {
    albums.iter().fold(String::new(), |mut out, a| {
        let _ = write!(
            out,
            "<a class=cell href=\"/album/{id}\"><img loading=lazy decoding=async width={size} height={size} \
src=\"{src}\" alt=\"\">\
<span class=ct>{title}</span><span class=ca>{artist}</span></a>",
            id = a.id,
            size = crate::covers::GRID,
            src = cover_url(a.id, crate::covers::GRID, versions),
            title = escape(&a.title),
            artist = escape(&a.artist_name),
        );
        out
    })
}

/// The "Load more" button, fetching `next` (a path with its query), or an
/// empty placeholder at the end of the listing.
fn more(next: Option<String>) -> String {
    match next {
        Some(next) => format!(
            "<div id=more class=more><button data-indicator:_more data-attr:disabled=\"$_more\" \
data-class:busy=\"$_more\" data-on:click=\"@get('{next}')\">Load more</button></div>"
        ),
        None => "<div id=more class=more></div>".into(),
    }
}

/// The codecs and genres the filters offer.
pub(super) type Options = Arc<(Vec<String>, Vec<String>)>;

/// How long the filter options are reused. Counting genres reads every track,
/// and a library changes far more slowly than people page through it.
const OPTIONS_TTL: std::time::Duration = std::time::Duration::from_secs(300);

fn filter_options(s: &UiState) -> Option<Options> {
    let mut held = s.options.lock().ok()?;
    if let Some((at, options)) = held.as_ref()
        && at.elapsed() < OPTIONS_TTL
    {
        return Some(options.clone());
    }
    let db = open(&s.pool)?;
    let options = Arc::new((
        queries::album_codecs(&db.conn).unwrap_or_default(),
        queries::genres(&db.conn, GENRES_OFFERED).unwrap_or_default(),
    ));
    *held = Some((std::time::Instant::now(), options.clone()));
    Some(options)
}

const ICON_SHARE: &str = "<svg viewBox=\"0 0 24 24\" aria-hidden=true>\
     <path d=\"M12 3l4.5 4.5h-3.5v7h-2v-7H7.5zM5 12h2v7h10v-7h2v9H5z\"/></svg>";

/// Share this track: its album, cued to it. Only where the page has a
/// `#share-result` to show the link in.
fn share_track_button(t: &TrackRow) -> String {
    match t.album_id {
        Some(album) => format!(
            "<button class=quiet data-indicator:_sharing data-attr:disabled=\"$_sharing\" \
data-on:click=\"@post('/album/{album}/share?track={id}')\" aria-label=\"Share this track\" \
title=\"Share this track\">{ICON_SHARE}</button>",
            id = t.id
        ),
        None => String::new(),
    }
}

/// A track row. `album` is set where the row stands alone (search) and names
/// the record it comes from.
fn track_row(
    t: &TrackRow,
    n: usize,
    show_artist: bool,
    album: bool,
    versions: &Versions,
    share: bool,
) -> String {
    let mut sub = Vec::new();
    if show_artist {
        sub.push(escape(&t.artist_name));
    }
    if album {
        sub.push(escape(&t.album_title));
    }
    let sub = if sub.is_empty() {
        String::new()
    } else {
        format!("<small>{}</small>", sub.join(" · "))
    };
    format!(
        "<li tabindex=0 data-id={id} data-dur={secs} data-title=\"{title}\" data-artist=\"{artist}\" \
data-album=\"{album_title}\" data-album-id={album_id} data-cover=\"{cover}\"><span class=n>{n}</span>\
<span class=t>{title}{sub}</span><span class=d>{dur}</span>{share}\
<button class=\"quiet add\" data-act=add aria-label=\"Add to queue\" title=\"Add to queue\">+</button></li>",
        id = t.id,
        secs = t.duration_ms.unwrap_or(0) / 1000,
        title = escape(&t.title),
        artist = escape(&t.artist_name),
        album_title = escape(&t.album_title),
        album_id = t.album_id.unwrap_or(0),
        cover = t
            .album_id
            .map(|a| cover_url(a, crate::covers::LARGE, versions))
            .unwrap_or_default(),
        dur = duration(t.duration_ms),
        share = if share {
            share_track_button(t)
        } else {
            String::new()
        },
    )
}

/// One page of albums as `b` narrows and orders them, and the URL of the next
/// page if there is one.
fn album_page(
    s: &UiState,
    user: i64,
    b: &Browse,
) -> Option<(Vec<AlbumRow>, Option<String>, Versions)> {
    let db = open(&s.pool)?;
    let mut albums = queries::list_albums(&db.conn, &b.albums(user, ALBUMS_PAGE + 1)).ok()?;
    let next = (albums.len() > ALBUMS_PAGE as usize)
        .then(|| format!("/albums/more?{}", b.query(b.offset + ALBUMS_PAGE)));
    albums.truncate(ALBUMS_PAGE as usize);
    let versions = album_versions(&db.conn, &albums);
    Some((albums, next, versions))
}

pub(super) async fn albums(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(b): Query<Browse>,
    headers: HeaderMap,
) -> Response {
    let b = b.seeded();
    let (st, bb, id) = (s.clone(), b.clone(), user.user_id);
    let found = blocking(move || Some((album_page(&st, id, &bb)?, filter_options(&st)?))).await;
    let Some(((albums, next, versions), options)) = found else {
        return unavailable();
    };
    let grid = if albums.is_empty() {
        "<p class=empty>No albums match.</p>".to_owned()
    } else {
        format!(
            "<div class=grid id=albums>{}</div>{}",
            cells(&albums, &versions),
            more(next)
        )
    };
    let inner = format!(
        "<h1>Albums</h1>{}{grid}",
        browse::toolbar(&b, "/albums", false, &options.0, &options.1)
    );
    respond(&s, &headers, &user, "Albums", "albums", &inner)
}

pub(super) async fn albums_more(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(b): Query<Browse>,
) -> Response {
    let Some((albums, next, versions)) = blocking(move || album_page(&s, user.user_id, &b)).await
    else {
        return unavailable();
    };
    let mut out = Vec::new();
    if !albums.is_empty() {
        out.push(patch(
            &cells(&albums, &versions),
            Some(("#albums", "append")),
        ));
    }
    out.push(patch(&more(next), None));
    events(out)
}

pub(super) async fn album(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let st = s.clone();
    let found = blocking(move || {
        let db = open(&st.pool)?;
        let album = queries::get_album(&db.conn, id).ok()??;
        let tracks = queries::tracks_for_album(&db.conn, id).ok()?;
        let versions = cover_versions(&db.conn, &[id]);
        Some((album, tracks, versions))
    })
    .await;
    let Some((album, tracks, versions)) = found else {
        return not_found();
    };
    let can_share = user.role.has_permission(Role::User);
    let discs = tracks
        .iter()
        .map(|t| t.disc.unwrap_or(1))
        .collect::<std::collections::BTreeSet<_>>();
    let mut rows = String::new();
    let mut disc = None;
    for (i, t) in tracks.iter().enumerate() {
        if discs.len() > 1 && disc != Some(t.disc.unwrap_or(1)) {
            disc = Some(t.disc.unwrap_or(1));
            let _ = write!(rows, "<li class=disc>Disc {}</li>", t.disc.unwrap_or(1));
        }
        let n = t.track_number.map_or(i + 1, |n| n as usize);
        rows.push_str(&track_row(
            t,
            n,
            t.artist_name != album.artist_name,
            false,
            &versions,
            can_share,
        ));
    }
    let total: i64 = tracks.iter().filter_map(|t| t.duration_ms).sum();
    let mut sub = vec![format!(
        "<a href=\"/artist/{}\">{}</a>",
        album.artist_id,
        escape(&album.artist_name)
    )];
    let y = year(album.date.as_deref());
    if !y.is_empty() {
        sub.push(escape(y));
    }
    sub.push(format!(
        "{} track{}",
        tracks.len(),
        if tracks.len() == 1 { "" } else { "s" }
    ));
    sub.push(duration(Some(total)));
    if let Some(codec) = &album.codec {
        sub.push(escape(codec));
    }
    let share = if can_share {
        format!(
            "<button data-indicator:_sharing data-attr:disabled=\"$_sharing\" data-class:busy=\"$_sharing\" \
data-on:click=\"@post('/album/{}/share')\">Share</button>",
            album.id
        )
    } else {
        String::new()
    };
    let inner = format!(
        "<header class=hero><img class=cover src=\"{cover}\" width={large} height={large} alt=\"\"><div class=info>\
<p class=kicker>Album</p><h1>{title}</h1><p class=sub>{sub}</p><div class=actions>\
<button class=primary data-act=play>Play</button><button data-act=shuffle>Shuffle</button>\
<button data-act=queue>Add to queue</button>{share}</div><div id=share-result></div></div></header>\
<ol class=tracks data-context=album>{rows}</ol>",
        cover = cover_url(album.id, crate::covers::LARGE, &versions),
        large = crate::covers::LARGE,
        title = escape(&album.title),
        sub = sub.join(" · "),
    );
    respond(&s, &headers, &user, &album.title, "album", &inner)
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub(super) struct Cue {
    track: Option<i64>,
}

/// Share the album, or with `?track=` the album cued to that track.
pub(super) async fn share_album(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Query(cue): Query<Cue>,
) -> Response {
    let target = ShareTarget::Album {
        album_id: id,
        start_track_id: cue.track,
    };
    share(s, user, target).await
}

pub(super) async fn share_artist(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> Response {
    share(s, user, ShareTarget::Artist(id)).await
}

/// Make a share link and show it in the page's `#share-result`.
async fn share(s: UiState, user: AuthUser, target: ShareTarget) -> Response {
    let result = if user.role.has_permission(Role::User) {
        blocking(move || {
            let db = open(&s.pool)?;
            let cfg = koan_core::config::Config::load().unwrap_or_default();
            Some(
                koan_core::helpers::create_native_share(&db, user.user_id, &cfg, &target, None)
                    .map(|o| o.url)
                    .map_err(|e| e.to_string()),
            )
        })
        .await
        .unwrap_or_else(|| Err("The library is unavailable.".into()))
    } else {
        Err("This account cannot make share links.".into())
    };
    let html = match result {
        Ok(url) => format!(
            "<div id=share-result class=share><input id=share-url readonly value=\"{url}\" aria-label=\"Share link\">\
<button data-on:click=\"navigator.clipboard.writeText(document.getElementById('share-url').value)\">Copy</button></div>",
            url = escape(&url)
        ),
        Err(e) => format!(
            "<div id=share-result class=\"share error\" role=alert>{}</div>",
            escape(&e)
        ),
    };
    events(vec![patch(&html, None)])
}

fn artist_list(artists: &[queries::ArtistRow]) -> String {
    artists.iter().fold(String::new(), |mut out, a| {
        let _ = write!(
            out,
            "<li><a href=\"/artist/{}\"><span class=t>{}</span><span class=d>{} album{}</span></a></li>",
            a.id,
            escape(&a.name),
            a.album_count,
            if a.album_count == 1 { "" } else { "s" }
        );
        out
    })
}

fn artist_page(
    s: &UiState,
    user: i64,
    b: &Browse,
) -> Option<(Vec<queries::ArtistRow>, Option<String>)> {
    let db = open(&s.pool)?;
    let mut artists = queries::list_artists(&db.conn, &b.artists(user, ARTISTS_PAGE + 1)).ok()?;
    let next = (artists.len() > ARTISTS_PAGE as usize)
        .then(|| format!("/artists/more?{}", b.query(b.offset + ARTISTS_PAGE)));
    artists.truncate(ARTISTS_PAGE as usize);
    Some((artists, next))
}

pub(super) async fn artists(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(b): Query<Browse>,
    headers: HeaderMap,
) -> Response {
    let (st, bb, id) = (s.clone(), b.clone(), user.user_id);
    let found = blocking(move || Some((artist_page(&st, id, &bb)?, filter_options(&st)?))).await;
    let Some(((artists, next), options)) = found else {
        return unavailable();
    };
    let list = if artists.is_empty() {
        "<p class=empty>No artists match.</p>".to_owned()
    } else {
        format!(
            "<ul class=list id=artists>{}</ul>{}",
            artist_list(&artists),
            more(next)
        )
    };
    let inner = format!(
        "<h1>Artists</h1>{}{list}",
        browse::toolbar(&b, "/artists", true, &options.0, &options.1)
    );
    respond(&s, &headers, &user, "Artists", "artists", &inner)
}

pub(super) async fn artists_more(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(b): Query<Browse>,
) -> Response {
    let Some((artists, next)) = blocking(move || artist_page(&s, user.user_id, &b)).await else {
        return unavailable();
    };
    let mut out = Vec::new();
    if !artists.is_empty() {
        out.push(patch(&artist_list(&artists), Some(("#artists", "append"))));
    }
    out.push(patch(&more(next), None));
    events(out)
}

pub(super) async fn artist(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let st = s.clone();
    let found = blocking(move || {
        let db = open(&st.pool)?;
        let artist = queries::get_artist(&db.conn, id).ok()??;
        let albums = queries::albums_for_artist(&db.conn, id).ok()?;
        let versions = album_versions(&db.conn, &albums);
        Some((artist, albums, versions))
    })
    .await;
    let Some((artist, albums, versions)) = found else {
        return not_found();
    };
    let share = if user.role.has_permission(Role::User) {
        format!(
            "<div class=actions><button data-indicator:_sharing data-attr:disabled=\"$_sharing\" \
data-class:busy=\"$_sharing\" data-on:click=\"@post('/artist/{}/share')\">Share</button></div>\
<div id=share-result></div>",
            artist.id
        )
    } else {
        String::new()
    };
    let inner = format!(
        "<p class=kicker>Artist</p><h1>{}</h1><p class=sub>{} album{} · {} tracks</p>{share}\
<div class=grid>{}</div>",
        escape(&artist.name),
        artist.album_count,
        if artist.album_count == 1 { "" } else { "s" },
        artist.track_count,
        cells(&albums, &versions)
    );
    respond(&s, &headers, &user, &artist.name, "artist", &inner)
}

pub(super) async fn playlists(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let st = s.clone();
    let found = blocking(move || {
        let db = open(&st.pool)?;
        queries::list_playlists(&db.conn, user.user_id).ok()
    })
    .await;
    let Some(lists) = found else {
        return unavailable();
    };
    let list = if lists.is_empty() {
        "<p class=empty>No playlists yet.</p>".to_owned()
    } else {
        let rows = lists.iter().fold(String::new(), |mut out, p| {
            let _ = write!(
                out,
                "<li><a href=\"/playlist/{}\"><span class=t>{}</span><span class=d>{} track{} · {}</span></a></li>",
                p.id,
                escape(&p.name),
                p.track_count,
                if p.track_count == 1 { "" } else { "s" },
                duration(Some(p.duration_ms)),
            );
            out
        });
        format!("<ul class=list>{rows}</ul>")
    };
    let inner = format!("<h1>Playlists</h1>{list}");
    respond(&s, &headers, &user, "Playlists", "playlists", &inner)
}

/// A playlist, laid out like an album so its rows play and queue the same way.
/// The caller's own playlists and anyone's public ones.
pub(super) async fn playlist(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let st = s.clone();
    let found = blocking(move || {
        let db = open(&st.pool)?;
        let me = queries::auth::resolve_user(&db.conn, user.user_id).ok()?;
        let list = queries::get_playlist(&db.conn, id)
            .ok()?
            .filter(|p| p.readable_by(me))?;
        let tracks = queries::playlist_tracks(&db.conn, id).ok()?;
        let versions = track_versions(&db.conn, &tracks);
        Some((list, tracks, versions))
    })
    .await;
    let Some((list, tracks, versions)) = found else {
        return not_found();
    };
    let rows: String = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| track_row(t, i + 1, true, true, &versions, false))
        .collect();
    let cover = tracks
        .iter()
        .find_map(|t| t.album_id)
        .map(|a| {
            format!(
                "<img class=cover src=\"{}\" width={large} height={large} alt=\"\">",
                cover_url(a, crate::covers::LARGE, &versions),
                large = crate::covers::LARGE,
            )
        })
        .unwrap_or_default();
    let mut sub = Vec::new();
    if let Some(owner) = list.owner.as_deref().filter(|o| !o.is_empty()) {
        sub.push(escape(owner));
    }
    sub.push(format!(
        "{} track{}",
        tracks.len(),
        if tracks.len() == 1 { "" } else { "s" }
    ));
    sub.push(duration(Some(list.duration_ms)));
    let comment = list
        .comment
        .as_deref()
        .filter(|c| !c.is_empty())
        .map(|c| format!("<p class=sub>{}</p>", escape(c)))
        .unwrap_or_default();
    let inner = format!(
        "<header class=hero>{cover}<div class=info><p class=kicker>Playlist</p><h1>{title}</h1>\
<p class=sub>{sub}</p>{comment}<div class=actions><button class=primary data-act=play>Play</button>\
<button data-act=shuffle>Shuffle</button><button data-act=queue>Add to queue</button></div></div></header>\
<ol class=tracks data-context=album>{rows}</ol>",
        title = escape(&list.name),
        sub = sub.join(" · "),
    );
    respond(&s, &headers, &user, &list.name, "playlist", &inner)
}

fn results(s: &UiState, q: &str) -> String {
    let q = q.trim();
    if q.is_empty() {
        return "<div id=results></div>".into();
    }
    let found = open(&s.pool).map(|db| {
        let albums = queries::list_albums(
            &db.conn,
            &AlbumQuery {
                search: Some(q),
                order: AlbumOrder::RecentlyAdded,
                limit: Some(12),
                ..Default::default()
            },
        )
        .unwrap_or_default();
        let artists = queries::list_artists(
            &db.conn,
            &ArtistQuery {
                search: Some(q),
                limit: Some(10),
                ..Default::default()
            },
        )
        .unwrap_or_default();
        let tracks = queries::search_tracks_paged(&db.conn, q, 50, 0).unwrap_or_default();
        let mut versions = album_versions(&db.conn, &albums);
        versions.extend(track_versions(&db.conn, &tracks));
        (albums, artists, tracks, versions)
    });
    let Some((albums, artists, tracks, versions)) = found else {
        return "<div id=results><p class=error>The library is unavailable.</p></div>".into();
    };
    if albums.is_empty() && artists.is_empty() && tracks.is_empty() {
        return format!(
            "<div id=results><p class=empty>Nothing matches “{}”.</p></div>",
            escape(q)
        );
    }
    let mut out = String::from("<div id=results>");
    if !artists.is_empty() {
        let _ = write!(
            out,
            "<h2>Artists</h2><ul class=list>{}</ul>",
            artist_list(&artists)
        );
    }
    if !albums.is_empty() {
        let _ = write!(
            out,
            "<h2>Albums</h2><div class=grid>{}</div>",
            cells(&albums, &versions)
        );
    }
    if !tracks.is_empty() {
        let rows: String = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| track_row(t, i + 1, true, true, &versions, false))
            .collect();
        let _ = write!(
            out,
            "<h2>Tracks</h2><ol class=tracks data-context=one>{rows}</ol>"
        );
    }
    out.push_str("</div>");
    out
}

pub(super) async fn search(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let q = params.get("q").cloned().unwrap_or_default();
    let st = s.clone();
    let query = q.clone();
    let found = blocking(move || Some(results(&st, &query)))
        .await
        .unwrap_or_default();
    // Without script the form is an ordinary GET; with it, results follow typing.
    let inner = format!(
        "<h1>Search</h1><form class=search action=\"/search\" method=get \
data-on:submit__prevent=\"@get('/search/results')\">\
<input type=search name=q value=\"{q}\" placeholder=\"Albums, artists, tracks\" autocomplete=off \
autocapitalize=none spellcheck=false enterkeyhint=search autofocus aria-label=Search data-bind:q \
data-init=\"$q && @get('/search/results')\" \
data-on:input__debounce.250ms=\"@get('/search/results')\"></form>{found}",
        q = escape(&q),
    );
    respond(&s, &headers, &user, "Search", "search", &inner)
}

/// Datastar sends its signals as JSON in `datastar`; a plain request sends `q`.
pub(super) async fn search_results(
    State(s): State<UiState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let q = params
        .get("datastar")
        .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
        .and_then(|v| v.get("q")?.as_str().map(str::to_owned))
        .or_else(|| params.get("q").cloned())
        .unwrap_or_default();
    let html = blocking(move || Some(results(&s, &q)))
        .await
        .unwrap_or_default();
    events(vec![patch(&html, None)])
}

/// The queue lives in the browser, so the page is a frame the script fills.
pub(super) async fn queue(
    State(s): State<UiState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let inner = format!(
        "<header class=\"hero now-playing\"><img class=cover data-np=cover alt=\"\" hidden>\
<div class=info><p class=kicker>Now playing</p><h1 data-np=title>Nothing playing</h1>\
<p class=sub><span data-np=artist></span> <a data-np=album href=\"/albums\"></a></p>\
<div class=controls>{}</div>{SCRUB}</div></header>\
<div class=queue-head><h2>Up next</h2><button class=quiet data-act=clear>Clear</button></div>\
<ol id=queue-list class=tracks></ol>",
        buttons()
    );
    respond(&s, &headers, &user, "Queue", "queue", &inner)
}

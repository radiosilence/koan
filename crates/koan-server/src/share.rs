//! Public share links: `/share/{id}`, and the audio and cover under it.
//!
//! The only routes a server answers without a login, so they answer for the
//! share's own tracks and nothing else. Tracks are addressed by their place in
//! the share, never by library id, so a link cannot be walked to anything it
//! does not name. An unknown, expired or revoked id is the same 404, so a
//! visitor learns nothing from trying ids. Only local files are served: a
//! standalone server proxies for nobody.
//!
//! The page is plain HTML that works without script; with it, koan's browser
//! player (`assets/player.js`, shared with the web UI) plays the tracks
//! gaplessly. Its CSP allows this server's own scripts and nothing else.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use koan_core::db::connection::Database;
use koan_core::db::pool::{Handle, Pool};
use koan_core::db::queries::shares::{ShareKind, ShareRow};
use koan_core::db::queries::{self, AlbumRow, ArtistRow, TrackRow};

/// The page's own script and stylesheet, from this server and nowhere else;
/// `connect-src` is for the player fetching the tracks it decodes.
const PAGE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
     media-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The gapless queue player, shared with the web UI.
pub(crate) const ENGINE_JS: &str = include_str!("../assets/player.js");
const PLAYER_JS: &str = include_str!("../assets/share.js");
const PAGE_CSS: &str = include_str!("../assets/share.css");

#[derive(Clone)]
struct ShareState {
    pool: Arc<Pool>,
    /// `sharing.public_url`: link previews need absolute addresses.
    public_url: Option<String>,
    covers: std::sync::Arc<crate::covers::Covers>,
}

pub fn router(
    pool: Arc<Pool>,
    public_url: Option<String>,
    covers: std::sync::Arc<crate::covers::Covers>,
) -> axum::Router {
    axum::Router::new()
        .route(
            "/share/assets/share.js",
            get(|| async { asset(PLAYER_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/share/assets/player.js",
            get(|| async { asset(ENGINE_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/share/assets/share.css",
            get(|| async { asset(PAGE_CSS, "text/css; charset=utf-8") }),
        )
        .route(
            "/share/assets/{name}",
            get(|Path(name): Path<String>| async move { icon(&name).unwrap_or_else(not_found) }),
        )
        .route("/share/{id}", get(page))
        .route("/share/{id}/cover", get(cover))
        .route("/share/{id}/{n}", get(track))
        .route("/share/{id}/{n}/cover", get(track_cover))
        .with_state(ShareState {
            pool,
            public_url: public_url.filter(|u| !u.trim().is_empty()),
            covers,
        })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A live share and its tracks in shared order, or `None` for anything a
/// visitor should not be able to tell apart from a share that never existed.
fn live<'a>(pool: &'a Pool, id: &str) -> Option<(Handle<'a>, ShareRow, Vec<TrackRow>)> {
    // Ids are 32 hex characters; anything else is not worth a query.
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let db = pool.get().ok()?;
    let share = queries::shares::get_share(&db.conn, id).ok()??;
    if !share.is_live(now()) {
        return None;
    }
    let rows = queries::tracks_by_ids(&db.conn, &share.track_ids).ok()?;
    let tracks = share
        .track_ids
        .iter()
        .filter_map(|id| rows.iter().find(|t| t.id == *id).cloned())
        .collect();
    Some((db, share, tracks))
}

/// The app icon, as the browser tab and home-screen icons of the web UI and
/// the share pages. The same images koan.rocks uses.
pub(crate) fn icon(name: &str) -> Option<Response> {
    let bytes: &'static [u8] = match name {
        "icon-32.png" => include_bytes!("../assets/icon-32.png"),
        "icon-192.png" => include_bytes!("../assets/icon-192.png"),
        "apple-touch-icon.png" => include_bytes!("../assets/apple-touch-icon.png"),
        _ => return None,
    };
    Some(
        (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
    )
}

/// `<link>`s for the icons, served under `base` (`/ui/assets` or `/share/assets`).
pub(crate) fn icon_links(base: &str) -> String {
    format!(
        "<link rel=icon type=image/png sizes=32x32 href=\"{base}/icon-32.png\">\
<link rel=icon type=image/png sizes=192x192 href=\"{base}/icon-192.png\">\
<link rel=apple-touch-icon href=\"{base}/apple-touch-icon.png\">"
    )
}

pub(crate) fn asset(body: &'static str, kind: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, kind),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
        .into_response()
}

pub(crate) fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CACHE_CONTROL, "no-store")],
        "Not found",
    )
        .into_response()
}

pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok().flatten()
}

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn duration(ms: Option<i64>) -> String {
    ms.map(|ms| format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60))
        .unwrap_or_default()
}

/// What the page shows besides the tracks, read when it is served: the album
/// or artist the share is a slice of, and the albums its tracks come from.
struct Subject {
    artist: Option<ArtistRow>,
    albums: Vec<AlbumRow>,
}

fn subject(db: &Database, share: &ShareRow, tracks: &[TrackRow]) -> Subject {
    let artist = match (share.slice.kind, share.slice.subject_id) {
        (ShareKind::Artist, Some(id)) => queries::get_artist(&db.conn, id).ok().flatten(),
        _ => None,
    };
    let mut ids: Vec<i64> = tracks.iter().filter_map(|t| t.album_id).collect();
    ids.dedup();
    let albums = ids
        .iter()
        .filter_map(|id| queries::get_album(&db.conn, *id).ok().flatten())
        .collect();
    Subject { artist, albums }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn year(date: Option<&str>) -> Option<&str> {
    date.and_then(|d| d.get(..4))
}

/// A `<meta>` for link previews. OpenGraph uses `property`, Twitter `name`.
fn meta(attr: &str, key: &str, content: &str) -> String {
    format!("<meta {attr}=\"{key}\" content=\"{}\">", escape(content))
}

/// The page: what was shared, shown the way the app shows it, and the player.
/// Each row carries what the player needs in data attributes; without script,
/// each row is a link. The link-preview tags need absolute URLs, so they are
/// only complete when `sharing.public_url` is set.
fn render(
    id: &str,
    share: &ShareRow,
    tracks: &[TrackRow],
    subject: &Subject,
    public_url: Option<&str>,
) -> String {
    // The slice as recorded, unless what it names has since left the library.
    let album = match share.slice.kind {
        ShareKind::Album => subject
            .albums
            .first()
            .filter(|a| Some(a.id) == share.slice.subject_id),
        _ => None,
    };
    let artist = subject.artist.as_ref();
    let one_album = tracks
        .first()
        .filter(|f| tracks.iter().all(|t| t.album_id == f.album_id))
        .map(|f| (f.album_title.clone(), f.album_artist_name.clone()));
    let total: i64 = tracks.iter().filter_map(|t| t.duration_ms).sum();
    let count = plural(tracks.len(), "track");
    let start = share
        .slice
        .start_track_id
        .and_then(|s| tracks.iter().position(|t| t.id == s));
    let note = share.description.clone().filter(|d| !d.trim().is_empty());

    let (title, sub, og_type, og_title, og_desc) = if let Some(album) = album {
        let mut sub = vec![album.artist_name.clone()];
        sub.extend(year(album.date.as_deref()).map(str::to_owned));
        sub.push(count.clone());
        sub.push(duration(Some(total)));
        match start.map(|i| &tracks[i]) {
            Some(t) => (
                album.title.clone(),
                sub,
                "music.song",
                format!("{} · {}", t.title, t.artist_name),
                format!("From {} by {}", album.title, album.artist_name),
            ),
            None => (
                album.title.clone(),
                sub.clone(),
                "music.album",
                format!("{} · {}", album.title, album.artist_name),
                sub[1..].join(" · "),
            ),
        }
    } else if let Some(artist) = artist {
        let sub = vec![
            plural(subject.albums.len(), "album"),
            count.clone(),
            duration(Some(total)),
        ];
        (
            artist.name.clone(),
            sub.clone(),
            "profile",
            artist.name.clone(),
            sub.join(" · "),
        )
    } else {
        let title = note
            .clone()
            .or_else(|| one_album.as_ref().map(|(a, _)| a.clone()))
            .unwrap_or_else(|| count.clone());
        let mut sub = Vec::new();
        sub.extend(one_album.as_ref().map(|(_, artist)| artist.clone()));
        sub.push(count.clone());
        sub.push(duration(Some(total)));
        (
            title.clone(),
            sub.clone(),
            "music.playlist",
            title,
            sub.join(" · "),
        )
    };
    // A loose list is titled by its note; a slice keeps the note beside it.
    let note = note
        .filter(|n| (album.is_some() || artist.is_some()) && *n != title)
        .map(|n| format!("<p class=note>{}</p>", escape(&n)))
        .unwrap_or_default();

    let mut preview = vec![
        meta("property", "og:site_name", "koan"),
        meta("property", "og:type", og_type),
        meta("property", "og:title", &og_title),
        meta("property", "og:description", &og_desc),
        meta("name", "description", &og_desc),
        meta("name", "twitter:card", "summary_large_image"),
        meta("name", "twitter:title", &og_title),
        meta("name", "twitter:description", &og_desc),
    ];
    if let Some(base) = public_url.map(|u| u.trim_end_matches('/')) {
        let image = format!("{base}/share/{id}/cover");
        preview.push(meta(
            "property",
            "og:url",
            &koan_core::helpers::share_url(base, id),
        ));
        preview.push(meta("property", "og:image", &image));
        preview.push(meta("name", "twitter:image", &image));
    }

    // An artist's album shows the album artist once, in its heading; a loose
    // list names everyone.
    let row = |i: usize, t: &TrackRow| {
        let credited = if one_album.is_some() || artist.is_some() {
            t.artist_name != t.album_artist_name
        } else {
            true
        };
        let small = if credited {
            format!("<small>{}</small>", escape(&t.artist_name))
        } else {
            String::new()
        };
        let n = match (album.is_some() || artist.is_some(), t.track_number) {
            (true, Some(n)) => n as usize,
            _ => i + 1,
        };
        format!(
            "<li tabindex=0 data-src=\"/share/{id}/{pos}\" data-dur=\"{secs}\" data-title=\"{title}\" \
             data-artist=\"{art}\" data-album=\"{alb}\"><span class=n>{n}</span><span class=t>{title}{small}</span>\
             <span class=d>{dur}</span></li>",
            pos = i + 1,
            secs = t.duration_ms.unwrap_or(0) / 1000,
            title = escape(&t.title),
            art = escape(&t.artist_name),
            alb = escape(&t.album_title),
            dur = duration(t.duration_ms),
        )
    };
    let body = if artist.is_some() {
        // One section per album, in the order shared, which is release order.
        let mut out = String::new();
        let mut i = 0;
        while i < tracks.len() {
            let album_id = tracks[i].album_id;
            let end = tracks[i..]
                .iter()
                .position(|t| t.album_id != album_id)
                .map_or(tracks.len(), |k| i + k);
            let info = subject.albums.iter().find(|a| Some(a.id) == album_id);
            let mut sub: Vec<String> = info
                .and_then(|a| year(a.date.as_deref()))
                .map(str::to_owned)
                .into_iter()
                .collect();
            sub.push(plural(end - i, "track"));
            let rows: String = (i..end).map(|k| row(k, &tracks[k])).collect();
            out.push_str(&format!(
                "<section class=album><header><img class=art src=\"/share/{id}/{first}/cover\" alt=\"\" loading=lazy>\
                 <div><h2>{title}</h2><p class=sub>{sub}</p></div></header><ol class=tracks>{rows}</ol></section>",
                first = i + 1,
                title = escape(&tracks[i].album_title),
                sub = escape(&sub.join(" · ")),
            ));
            i = end;
        }
        out
    } else {
        let rows: String = tracks.iter().enumerate().map(|(i, t)| row(i, t)).collect();
        format!("<ol class=tracks>{rows}</ol>")
    };
    let links: String = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            format!(
                "<a href=\"/share/{id}/{}\">{}</a><br>",
                i + 1,
                escape(&t.title)
            )
        })
        .collect();
    let kicker = if artist.is_some() {
        "Artist shared from koan"
    } else {
        "Shared from koan"
    };
    format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8>\
<meta name=viewport content=\"width=device-width,initial-scale=1,viewport-fit=cover\">\
<meta name=robots content=\"noindex,nofollow\"><title>{title}</title>{preview}\
{icons}<link rel=stylesheet href=\"/share/assets/share.css\"></head><body><main>\
<header class=hero><img id=cover class=cover src=\"/share/{id}/cover\" alt=\"\">\
<div class=info><p class=kicker>{kicker}</p><h1>{title}</h1><p class=sub>{sub}</p>{note}\
<div class=controls><button id=prev class=quiet aria-label=Previous>&#9198;</button>\
<button id=play class=primary>Play</button><button id=next class=quiet aria-label=Next>&#9197;</button></div>\
<div class=scrub><span id=pos>0:00</span><input id=seek type=range min=0 max=0 step=0.1 value=0 aria-label=Position>\
<span id=len>0:00</span></div></div></header>\
<div id=tracks data-start=\"{start}\">{body}</div><noscript><p>{links}</p></noscript></main>\
<script src=\"/share/assets/player.js\" defer></script>\
<script src=\"/share/assets/share.js\" defer></script></body></html>",
        title = escape(&title),
        preview = preview.concat(),
        icons = icon_links("/share/assets"),
        sub = escape(&sub.join(" · ")),
        start = start.map_or(-1, |i| i as i64),
    )
}

async fn page(State(s): State<ShareState>, Path(id): Path<String>) -> Response {
    let found = blocking(move || {
        let (db, share, tracks) = live(&s.pool, &id)?;
        let _ = queries::shares::record_visit(&db.conn, &id, now());
        let subject = subject(&db, &share, &tracks);
        Some(render(
            &id,
            &share,
            &tracks,
            &subject,
            s.public_url.as_deref(),
        ))
    })
    .await;
    let Some(html) = found else {
        return not_found();
    };
    let mut resp = (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        html,
    )
        .into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(PAGE_CSP),
    );
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    resp
}

async fn track(
    State(s): State<ShareState>,
    Path((id, n)): Path<(String, usize)>,
    headers: HeaderMap,
) -> Response {
    let path = blocking(move || {
        let (_, _, tracks) = live(&s.pool, &id)?;
        let t = tracks.get(n.checked_sub(1)?)?;
        crate::subsonic::track_file_path(t).map(PathBuf::from)
    })
    .await;
    let Some(path) = path else {
        return not_found();
    };
    match crate::subsonic::serve_local_file(&path, &headers).await {
        Ok(mut resp) => {
            resp.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=3600"),
            );
            resp
        }
        Err(_) => not_found(),
    }
}

/// The cover at the size link previews and the page's header want.
async fn cover(State(s): State<ShareState>, Path(id): Path<String>) -> Response {
    let art = blocking(move || {
        let (_, _, tracks) = live(&s.pool, &id)?;
        s.covers.cover(&tracks, crate::covers::LARGE)
    })
    .await;
    jpeg(art, false)
}

/// The cover of the album track `n` comes from: an artist page's headings.
async fn track_cover(
    State(s): State<ShareState>,
    Path((id, n)): Path<(String, usize)>,
) -> Response {
    let art = blocking(move || {
        let (_, _, tracks) = live(&s.pool, &id)?;
        let t = tracks.get(n.checked_sub(1)?)?;
        s.covers
            .cover(std::slice::from_ref(t), crate::covers::SIZES[0])
    })
    .await;
    jpeg(art, false)
}

/// A cover from `Covers`. `immutable` when the URL carries the cover's
/// version, so the same URL can never name different bytes.
pub(crate) fn jpeg(art: Option<axum::body::Bytes>, immutable: bool) -> Response {
    let Some(bytes) = art else {
        return not_found();
    };
    let cache = if immutable {
        "private, max-age=31536000, immutable"
    } else {
        "private, max-age=86400"
    };
    (
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (header::CACHE_CONTROL, cache),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use koan_core::db::queries::TrackMeta;
    use koan_core::db::queries::shares::Slice;
    use koan_core::helpers::{ShareTarget, resolve_share};
    use tower::ServiceExt;

    fn test_covers(dir: &tempfile::TempDir) -> std::sync::Arc<crate::covers::Covers> {
        std::sync::Arc::new(crate::covers::Covers::new(dir.path().join("covers")))
    }

    fn meta(path: &std::path::Path, title: &str, n: i32) -> TrackMeta {
        TrackMeta {
            title: title.into(),
            artist: "Rrose".into(),
            album: "Hymn to Moisture".into(),
            album_artist: Some("Rrose".into()),
            track_number: Some(n),
            disc: Some(1),
            duration_ms: Some(125_000),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1000),
            genre: None,
            path: Some(path.to_string_lossy().into_owned()),
            date: Some("2019".into()),
            label: None,
            size_bytes: None,
            mtime: None,
            source: "local".into(),
            remote_id: None,
            remote_url: None,
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            album_mbid: None,
            album_added_at: None,
        }
    }

    /// A library of two tracks, one shared and one not, and a share of the first.
    fn setup() -> (tempfile::TempDir, axum::Router, String, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("koan.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();
        let shared = dir.path().join("a.flac");
        std::fs::write(&shared, b"0123456789").unwrap();
        let other = dir.path().join("b.flac");
        std::fs::write(&other, b"secret").unwrap();
        let a = queries::upsert_track(&db.conn, &meta(&shared, "Wet <Moss> & Stone", 1)).unwrap();
        let b = queries::upsert_track(&db.conn, &meta(&other, "Unshared", 2)).unwrap();
        let share =
            queries::shares::create_share(&db.conn, Slice::TRACKS, &[a], None, 0, None).unwrap();
        let covers = test_covers(&dir);
        (
            dir,
            router(Arc::new(Pool::new(db_path)), None, covers),
            share.id,
            b,
        )
    }

    async fn get(
        app: &axum::Router,
        uri: &str,
        range: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut req = Request::builder().uri(uri);
        if let Some(r) = range {
            req = req.header(header::RANGE, r);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (parts.status, parts.headers, bytes)
    }

    #[tokio::test]
    async fn the_page_lists_the_share_escaped_and_forbids_scripts() {
        let (_dir, app, id, _) = setup();
        let (status, headers, body) = get(&app, &format!("/share/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        let html = String::from_utf8(body).unwrap();
        assert!(html.contains("Wet &lt;Moss&gt; &amp; Stone"));
        assert!(!html.contains("<Moss>"));
        assert!(!html.contains("Unshared"));
        assert!(html.contains(&format!("data-src=\"/share/{id}/1\"")));
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.starts_with("default-src 'none'"));
        assert!(
            csp.contains("script-src 'self'") && !csp.contains("unsafe"),
            "{csp}"
        );
        assert!(!html.contains("<script>"), "no inline script");
    }

    #[tokio::test]
    async fn only_the_shared_tracks_stream_and_ranges_work() {
        let (_dir, app, id, _) = setup();
        let (status, _, body) = get(&app, &format!("/share/{id}/1"), None).await;
        assert_eq!(
            (status, body.as_slice()),
            (StatusCode::OK, &b"0123456789"[..])
        );
        let (status, _, body) = get(&app, &format!("/share/{id}/1"), Some("bytes=2-4")).await;
        assert_eq!(
            (status, body.as_slice()),
            (StatusCode::PARTIAL_CONTENT, &b"234"[..])
        );
        // Position 2 does not exist in this share, though a second track exists
        // in the library; and position 0 is not a position.
        for n in ["2", "0", "x"] {
            let (status, _, _) = get(&app, &format!("/share/{id}/{n}"), None).await;
            assert_ne!(status, StatusCode::OK, "/share/{{id}}/{n}");
        }
    }

    #[tokio::test]
    async fn unknown_expired_and_revoked_shares_look_alike() {
        let (dir, app, id, _) = setup();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let expired =
            queries::shares::create_share(&db.conn, Slice::TRACKS, &[1], None, 0, Some(1)).unwrap();
        let revoked =
            queries::shares::create_share(&db.conn, Slice::TRACKS, &[1], None, 0, None).unwrap();
        queries::shares::delete_share(&db.conn, &revoked.id).unwrap();
        let unknown = "0".repeat(32);
        let mut answers = Vec::new();
        for gone in [&expired.id, &revoked.id, &unknown, &"nonsense".to_string()] {
            answers.push(get(&app, &format!("/share/{gone}"), None).await);
            let (status, _, _) = get(&app, &format!("/share/{gone}/1"), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
        assert!(
            answers
                .iter()
                .all(|(s, _, b)| *s == StatusCode::NOT_FOUND && b == &answers[0].2)
        );
        let (status, _, _) = get(&app, &format!("/share/{id}"), None).await;
        assert_eq!(status, StatusCode::OK, "the live one still works");
    }

    /// Two albums by one artist, released years apart, the first with a title
    /// that needs escaping; each file holds a few bytes of fake audio.
    struct Library {
        dir: tempfile::TempDir,
        db: Database,
        app: axum::Router,
        tracks: [i64; 3],
        artist: i64,
    }

    fn library() -> Library {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("koan.db");
        let db = Database::open(&db_path).unwrap();
        let track = |file: &str, title: &str, album: &str, n: i32, date: &str| {
            let path = dir.path().join(file);
            std::fs::write(&path, b"audio").unwrap();
            let mut m = meta(&path, title, n);
            m.album = album.into();
            m.date = Some(date.into());
            queries::upsert_track(&db.conn, &m).unwrap()
        };
        let later = track("c.flac", "Later One", "Later", 1, "2021");
        let t1 = track(
            "a.flac",
            "Wet \"Moss\"",
            "Hymn <to> \"Moisture\"",
            1,
            "2019",
        );
        let t2 = track(
            "b.flac",
            "Stone & Salt",
            "Hymn <to> \"Moisture\"",
            2,
            "2019",
        );
        let artist = queries::tracks_by_ids(&db.conn, &[t1]).unwrap()[0]
            .artist_id
            .unwrap();
        Library {
            app: router(
                Arc::new(Pool::new(db_path)),
                Some("https://koan.example/".into()),
                test_covers(&dir),
            ),
            dir,
            db,
            tracks: [t1, t2, later],
            artist,
        }
    }

    impl Library {
        fn share(&self, target: ShareTarget) -> String {
            let (slice, ids) = resolve_share(&self.db.conn, &target).unwrap();
            queries::shares::create_share(&self.db.conn, slice, &ids, None, 0, None)
                .unwrap()
                .id
        }

        async fn page(&self, id: &str) -> String {
            let (status, _, body) = get(&self.app, &format!("/share/{id}"), None).await;
            assert_eq!(status, StatusCode::OK);
            String::from_utf8(body).unwrap()
        }
    }

    fn og<'a>(html: &'a str, key: &str) -> &'a str {
        let at = html
            .find(&format!("property=\"{key}\" content=\""))
            .unwrap_or_else(|| panic!("no {key}"));
        let rest = &html[at + key.len() + 21..];
        &rest[..rest.find('"').unwrap()]
    }

    #[tokio::test]
    async fn a_track_shares_its_album_cued_to_it() {
        let lib = library();
        let [_, t2, _] = lib.tracks;
        let id = lib.share(ShareTarget::Tracks(vec![t2]));
        let html = lib.page(&id).await;
        assert!(html.contains("<h1>Hymn &lt;to&gt; &quot;Moisture&quot;</h1>"));
        assert!(
            html.contains("data-start=\"1\""),
            "cued to the second track"
        );
        assert_eq!(html.matches("data-src=").count(), 2, "the whole album");
        assert_eq!(og(&html, "og:type"), "music.song");
        assert_eq!(og(&html, "og:title"), "Stone &amp; Salt · Rrose");
        assert_eq!(
            og(&html, "og:description"),
            "From Hymn &lt;to&gt; &quot;Moisture&quot; by Rrose"
        );
        assert_eq!(
            og(&html, "og:image"),
            format!("https://koan.example/share/{id}/cover")
        );
        assert_eq!(
            og(&html, "og:url"),
            format!("https://koan.example/share/{id}")
        );
        assert!(html.contains("name=\"twitter:card\" content=\"summary_large_image\""));
        assert!(!html.contains("<to>") && !html.contains("\"Moss\""));
    }

    #[tokio::test]
    async fn an_album_share_is_the_album() {
        let lib = library();
        let album_id = queries::tracks_by_ids(&lib.db.conn, &[lib.tracks[0]]).unwrap()[0]
            .album_id
            .unwrap();
        let id = lib.share(ShareTarget::Album {
            album_id,
            start_track_id: None,
        });
        let html = lib.page(&id).await;
        assert!(html.contains("data-start=\"-1\""));
        assert_eq!(og(&html, "og:type"), "music.album");
        assert_eq!(
            og(&html, "og:title"),
            "Hymn &lt;to&gt; &quot;Moisture&quot; · Rrose"
        );
        assert_eq!(og(&html, "og:description"), "2019 · 2 tracks · 4:10");
    }

    #[tokio::test]
    async fn an_artist_share_is_their_albums_in_release_order() {
        let lib = library();
        let id = lib.share(ShareTarget::Artist(lib.artist));
        let html = lib.page(&id).await;
        assert_eq!(og(&html, "og:type"), "profile");
        assert_eq!(og(&html, "og:title"), "Rrose");
        assert_eq!(og(&html, "og:description"), "2 albums · 3 tracks · 6:15");
        assert_eq!(html.matches("<section class=album>").count(), 2);
        let first = html.find("Hymn &lt;to&gt;").unwrap();
        assert!(first < html.find("<h2>Later</h2>").unwrap());
        // Each album heading's art is addressed through the share, by position.
        assert!(html.contains(&format!("src=\"/share/{id}/3/cover\"")));
        for n in ["3", "4"] {
            let (status, _, _) = get(&lib.app, &format!("/share/{id}/{n}/cover"), None).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "no art in fake files, nor a 4th track"
            );
        }
    }

    #[tokio::test]
    async fn several_tracks_stay_a_list_and_previews_need_a_public_url() {
        let lib = library();
        let [t1, _, later] = lib.tracks;
        let id = lib.share(ShareTarget::Tracks(vec![later, t1]));
        let html = lib.page(&id).await;
        assert_eq!(og(&html, "og:type"), "music.playlist");
        assert!(html.contains("<h1>2 tracks</h1>"));
        assert_eq!(html.matches("<section").count(), 0);

        let bare = router(
            Arc::new(Pool::new(lib.dir.path().join("koan.db"))),
            None,
            test_covers(&lib.dir),
        );
        let (_, _, body) = get(&bare, &format!("/share/{id}"), None).await;
        let html = String::from_utf8(body).unwrap();
        assert!(html.contains("property=\"og:title\""));
        assert!(!html.contains("og:image") && !html.contains("og:url"));
    }
}

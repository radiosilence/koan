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

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use koan_core::db::connection::Database;
use koan_core::db::queries::{self, TrackRow, shares::ShareRow};

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
    db_path: PathBuf,
    covers: std::sync::Arc<crate::covers::Covers>,
}

pub fn router(db_path: PathBuf, covers: std::sync::Arc<crate::covers::Covers>) -> axum::Router {
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
        .route("/share/{id}", get(page))
        .route("/share/{id}/cover", get(cover))
        .route("/share/{id}/{n}", get(track))
        .with_state(ShareState { db_path, covers })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A live share and its tracks in shared order, or `None` for anything a
/// visitor should not be able to tell apart from a share that never existed.
fn live(db_path: &std::path::Path, id: &str) -> Option<(Database, ShareRow, Vec<TrackRow>)> {
    // Ids are 32 hex characters; anything else is not worth a query.
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let db = Database::open(db_path).ok()?;
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

/// The page: what was shared, and the player. The track list carries what
/// the player needs in data attributes; without script, each row is a link.
fn render(id: &str, share: &ShareRow, tracks: &[TrackRow]) -> String {
    let album = tracks
        .first()
        .filter(|f| tracks.iter().all(|t| t.album_id == f.album_id))
        .map(|f| (f.album_title.clone(), f.album_artist_name.clone()));
    let title = share
        .description
        .clone()
        .filter(|d| !d.trim().is_empty())
        .or_else(|| album.as_ref().map(|(a, _)| a.clone()))
        .unwrap_or_else(|| format!("{} tracks", tracks.len()));
    let total: i64 = tracks.iter().filter_map(|t| t.duration_ms).sum();
    let mut sub = Vec::new();
    if let Some((_, artist)) = &album {
        sub.push(escape(artist));
    }
    sub.push(format!(
        "{} track{}",
        tracks.len(),
        if tracks.len() == 1 { "" } else { "s" }
    ));
    sub.push(duration(Some(total)));
    let rows: String = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let artist = if album.is_some() && t.artist_name == t.album_artist_name {
                String::new()
            } else {
                format!("<small>{}</small>", escape(&t.artist_name))
            };
            format!(
                "<li tabindex=0 data-src=\"/share/{id}/{n}\" data-dur=\"{secs}\" data-title=\"{title}\" \
                 data-artist=\"{art}\"><span class=n>{n}</span><span class=t>{title}{artist}</span>\
                 <span class=d>{dur}</span></li>",
                n = i + 1,
                secs = t.duration_ms.unwrap_or(0) / 1000,
                title = escape(&t.title),
                art = escape(&t.artist_name),
                dur = duration(t.duration_ms),
            )
        })
        .collect();
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
    format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8>\
<meta name=viewport content=\"width=device-width,initial-scale=1,viewport-fit=cover\">\
<meta name=robots content=\"noindex,nofollow\"><title>{title}</title>\
<link rel=stylesheet href=\"/share/assets/share.css\"></head><body><main>\
<header class=hero><img id=cover class=cover src=\"/share/{id}/cover\" alt=\"\">\
<div class=info><p class=kicker>Shared from koan</p><h1>{title}</h1><p class=sub>{sub}</p>\
<div class=controls><button id=prev class=quiet aria-label=Previous>&#9198;</button>\
<button id=play class=primary>Play</button><button id=next class=quiet aria-label=Next>&#9197;</button></div>\
<div class=scrub><span id=pos>0:00</span><input id=seek type=range min=0 max=0 step=0.1 value=0 aria-label=Position>\
<span id=len>0:00</span></div></div></header>\
<ol id=tracks>{rows}</ol><noscript><p>{links}</p></noscript></main>\
<script src=\"/share/assets/player.js\" defer></script>\
<script src=\"/share/assets/share.js\" defer></script></body></html>",
        title = escape(&title),
        sub = sub.join(" · "),
    )
}

async fn page(State(s): State<ShareState>, Path(id): Path<String>) -> Response {
    let found = blocking(move || {
        let (db, share, tracks) = live(&s.db_path, &id)?;
        let _ = queries::shares::record_visit(&db.conn, &id, now());
        Some(render(&id, &share, &tracks))
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
        let (_, _, tracks) = live(&s.db_path, &id)?;
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
        let (_, _, tracks) = live(&s.db_path, &id)?;
        s.covers.cover(&tracks, crate::covers::LARGE)
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
        let share = queries::shares::create_share(&db.conn, &[a], None, 0, None).unwrap();
        let covers = test_covers(&dir);
        (dir, router(db_path, covers), share.id, b)
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
        let expired = queries::shares::create_share(&db.conn, &[1], None, 0, Some(1)).unwrap();
        let revoked = queries::shares::create_share(&db.conn, &[1], None, 0, None).unwrap();
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
}

//! The escaping guard: the real UI over a library and accounts whose every
//! field a person or a remote server controls holds markup-shaped text,
//! requested through the router as a signed-in browser would, every page whole
//! and in place, every page's live stream, the regions and answers actions
//! send, and the pages outside the shell. `askama_datastar::escaping::Guard`
//! checks each answer for the templates' own markup escaped (a fragment
//! spliced in as text), text escaped twice, or text left as markup; where a
//! page shows the text, it must be there as a page escapes it. A page, a
//! stream or a field a person controls added to the UI belongs here.

use std::time::Duration;

use askama_datastar::escaping::{Guard, escaped, send};

use super::fixture::*;

/// What every field a person or a server controls is filled with.
const HOSTILE: &str = r#"<em>hostile "q" & 'x'</em>"#;

/// How long a live stream is read before it is cut off.
const WAIT: Duration = Duration::from_millis(400);

/// The ids of a patch's top-level elements and the page's elements, each
/// written once.
fn twice(answer: &str) -> Vec<String> {
    let mut seen = std::collections::HashMap::<&str, usize>::new();
    for part in answer.split(" id=\"").skip(1) {
        if let Some(id) = part.split('"').next() {
            *seen.entry(id).or_default() += 1;
        }
    }
    seen.into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(id, n)| format!("id {id:?} written {n} times"))
        .collect()
}

#[tokio::test]
async fn every_answer_escapes_what_people_and_servers_control_exactly_once() {
    let f = setup(true);
    let (track, playlist) = {
        let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
        let file = f.dir.path().join("hostile.flac");
        std::fs::write(&file, b"0123456789").unwrap();
        let mut m = meta(&file, HOSTILE, 2);
        m.artist = HOSTILE.into();
        m.album = HOSTILE.into();
        m.album_artist = Some(HOSTILE.into());
        m.genre = Some(HOSTILE.into());
        m.codec = Some(HOSTILE.into());
        let track = queries::upsert_track(&db.conn, &m).unwrap();
        let playlist = queries::create_playlist(&db.conn, 1, HOSTILE, Some(HOSTILE)).unwrap();
        queries::set_playlist_tracks(&db.conn, playlist, &[track, f.track_id]).unwrap();
        queries::add_favourite(&db.conn, 1, track).unwrap();
        queries::record_play(&db.conn, 1, track, Some(1000)).unwrap();
        koan_core::db::queries::api_keys::create_api_key(&db.conn, 1, HOSTILE).unwrap();
        (track, playlist)
    };
    let album = {
        let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
        queries::tracks_by_ids(&db.conn, &[track]).unwrap()[0]
            .album_id
            .unwrap()
    };
    let artist = {
        let db = Database::open(&f.dir.path().join("koan.db")).unwrap();
        queries::tracks_by_ids(&db.conn, &[track]).unwrap()[0]
            .artist_id
            .unwrap()
    };
    let guard = Guard::new(&[HOSTILE])
        .templates(concat!(env!("CARGO_MANIFEST_DIR"), "/templates"))
        .unwrap()
        .templates(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../koan-kit-web/templates"
        ))
        .unwrap();
    let cookie = format!("koan_access={}", access_token(&f.state));
    let request = |method: &str, uri: &str, datastar: bool, body: &str| {
        let mut r = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, HOST)
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, &cookie);
        if datastar {
            r = r.header("datastar-request", "true");
        }
        if method == "POST" {
            r = r.header(header::CONTENT_TYPE, "application/json");
        }
        r.body(Body::from(body.to_owned())).unwrap()
    };
    let enc = |s: &str| form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let pages: Vec<(String, bool)> = vec![
        ("/albums".into(), true),
        (format!("/albums?q={}", enc(HOSTILE)), true),
        (format!("/album/{album}"), true),
        ("/artists".into(), true),
        (format!("/artist/{artist}"), true),
        ("/tracks".into(), true),
        (format!("/tracks?genre={}", enc(HOSTILE)), true),
        (format!("/track/{track}/properties"), true),
        ("/playlists".into(), true),
        (format!("/playlist/{playlist}"), true),
        (format!("/search?q={}", enc("hostile")), true),
        (format!("/search?q={}", enc(HOSTILE)), true),
        ("/queue".into(), false),
        ("/library".into(), false),
        ("/recent".into(), true),
        ("/favourites".into(), true),
        ("/history".into(), true),
        ("/account".into(), true),
        ("/assistants".into(), false),
        ("/scrobbling".into(), false),
    ];
    let mut found = Vec::new();
    for (url, shows) in &pages {
        for in_place in [false, true] {
            let (status, answer) = send(&f.app, request("GET", url, in_place, ""), WAIT).await;
            let how = if in_place { "in place" } else { "whole" };
            if !status.is_success() {
                found.push(format!("{url} {how}: {status}"));
            }
            found.extend(
                guard
                    .problems(&answer)
                    .iter()
                    .map(|p| format!("{url} {how}: {p}")),
            );
            found.extend(
                twice(&answer)
                    .into_iter()
                    .map(|p| format!("{url} {how}: {p}")),
            );
            if *shows && !answer.contains(&escaped(HOSTILE)) {
                found.push(format!("{url} {how}: the hostile text is missing"));
            }
        }
        let signals = serde_json::json!({ "url": url }).to_string();
        let live = format!("/ui/live?datastar={}", enc(&signals));
        let (_, answer) = send(&f.app, request("GET", &live, true, ""), WAIT).await;
        found.extend(
            guard
                .problems(&answer)
                .iter()
                .map(|p| format!("{url} live: {p}")),
        );
    }
    // What actions answer with: the queue's rows, a share, a heart, a key
    // made with a hostile name.
    let actions = [
        (
            "/queue/rows".to_owned(),
            format!(r#"{{"ids":[{track},{}]}}"#, f.track_id),
        ),
        (format!("/favourite/song/{track}?on=0"), "{}".to_owned()),
        (
            format!("/album/{album}/share?track={track}"),
            "{}".to_owned(),
        ),
    ];
    for (url, body) in &actions {
        let (_, answer) = send(&f.app, request("POST", url, true, body), WAIT).await;
        found.extend(
            guard
                .problems(&answer)
                .iter()
                .map(|p| format!("POST {url}: {p}")),
        );
        found.extend(
            twice(&answer)
                .into_iter()
                .map(|p| format!("POST {url}: {p}")),
        );
    }
    let key = Request::post("/account/keys")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &cookie)
        .header("datastar-request", "true")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("name={}", enc("<b>k</b> & 'q'"))))
        .unwrap();
    let (_, answer) = send(&f.app, key, WAIT).await;
    found.extend(
        guard
            .problems(&answer)
            .iter()
            .map(|p| format!("POST /account/keys: {p}")),
    );
    // The pages outside the shell.
    for url in ["/login", &format!("/login?next={}", enc(HOSTILE)), "/pair"] {
        let r = Request::get(url)
            .header(header::HOST, HOST)
            .body(Body::empty())
            .unwrap();
        let (_, answer) = send(&f.app, r, WAIT).await;
        found.extend(
            guard
                .problems(&answer)
                .iter()
                .map(|p| format!("{url}: {p}")),
        );
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

//! Cover art: `getCoverArt`, and `koanCovers`, many covers at one size from
//! the cover cache.

use super::*;

pub(super) async fn get_cover_art(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CoverArtParams>,
) -> Response {
    offload_response(move || {
        let json = params.auth.wants_json();
        match cover_art_inner(&state, &params) {
            Ok(resp) => resp,
            Err(e) => SubsonicResponse::error(json, &e),
        }
    })
    .await
}

pub(super) fn cover_art_inner(
    state: &AppState,
    params: &CoverArtParams,
) -> Result<Response, SubsonicError> {
    let db = authed_db(state, &params.auth)?;
    let (kind, id) = require_entity(&db, params.id.as_deref())?;
    // Snapped to the sizes `Covers` keeps; no size asks for the largest.
    let size = crate::covers::snap(Some(params.size.unwrap_or(u32::MAX)));
    let art = if kind == Some(EntityKind::Album) {
        // An album's cover from its own entry, touching no music file; the
        // tracks are read only the first time, or after the record changed.
        state.covers.forget_evicted(&db.conn);
        let mut db = Some(db);
        let mut missing = None;
        let mut unreadable = false;
        let art = state.covers.album_cover(id, size, || {
            let db = db.take()?;
            match cover_tracks(&db, kind, id) {
                Ok(groups) => {
                    drop(db);
                    // None of its files to read: nothing is known, and
                    // nothing kept.
                    if !groups.iter().any(|tracks| crate::covers::readable(tracks)) {
                        unreadable = true;
                        return None;
                    }
                    Some(
                        groups
                            .iter()
                            .find_map(|tracks| state.covers.cover(tracks, size)),
                    )
                }
                Err(e) => {
                    missing = Some(e);
                    None
                }
            }
        });
        if let Some(e) = missing {
            return Err(e);
        }
        (art, unreadable)
    } else {
        let groups = cover_tracks(&db, kind, id)?;
        drop(db);
        let art = groups
            .iter()
            .find_map(|tracks| state.covers.cover(tracks, size));
        let unreadable =
            art.is_none() && !groups.iter().any(|tracks| crate::covers::readable(tracks));
        (art, unreadable)
    };
    // None of the record's files can be read now (a volume not mounted):
    // not knowing is said as such, so a client asks again rather than
    // remembering a record with no art.
    let (art, unreadable) = art;
    if art.is_none() && unreadable {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "60")],
        )
            .into_response());
    }
    // A record without art is answered with the ensō, as Navidrome answers
    // its own placeholder, so a client always has an image to draw. It is
    // marked so koan's own clients can draw theirs instead, and kept only
    // briefly so a cover added later is picked up.
    match art {
        Some(bytes) => Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "max-age=86400"),
            ],
            bytes,
        )
            .into_response()),
        None => Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "max-age=300"),
                (
                    header::HeaderName::from_static(koan_core::remote::client::PLACEHOLDER_HEADER),
                    "1",
                ),
            ],
            crate::covers::placeholder(size),
        )
            .into_response()),
    }
}

/// The tracks whose art answers a `getCoverArt` id, as groups tried in turn:
/// an album's tracks, a song alone, or an artist's albums one by one. A
/// track's art is the cover image beside it or, without one, its embedded art.
pub(super) fn cover_tracks(
    db: &Database,
    kind: Option<EntityKind>,
    id: i64,
) -> Result<Vec<Vec<queries::TrackRow>>, SubsonicError> {
    let internal = |e: koan_core::db::connection::DbError| SubsonicError::internal(e.to_string());
    let groups = match kind {
        Some(EntityKind::Album) => {
            let tracks = queries::tracks_for_album(&db.conn, id).map_err(internal)?;
            if tracks.is_empty() {
                return Err(SubsonicError::not_found("Album"));
            }
            vec![tracks]
        }
        Some(EntityKind::Artist) => {
            // Every album's tracks in one query, then grouped in the order the
            // albums are tried.
            let albums: Vec<i64> = queries::albums_for_artist(&db.conn, id)
                .map_err(internal)?
                .iter()
                .map(|album| album.id)
                .collect();
            let mut tracks =
                queries::batch::tracks_for_albums(&db.conn, &albums).map_err(internal)?;
            let groups: Vec<_> = albums
                .iter()
                .filter_map(|album| tracks.remove(album))
                .filter(|tracks| !tracks.is_empty())
                .collect();
            // An artist credited only on others' records has no albums of
            // their own, and is answered with the placeholder like any
            // record without art.
            if groups.is_empty()
                && queries::get_artist(&db.conn, id)
                    .map_err(internal)?
                    .is_none()
            {
                return Err(SubsonicError::not_found("Artist"));
            }
            groups
        }
        Some(EntityKind::Song) | None => vec![vec![
            queries::get_track_row(&db.conn, id)
                .map_err(internal)?
                .ok_or_else(|| SubsonicError::not_found("Track"))?,
        ]],
    };
    Ok(groups)
}

/// How many covers `koanCovers` reads ahead of the frame being sent.
pub(super) const COVERS_AHEAD: usize = 4;

/// `koanCovers`: the covers of many records at one size, answered from what
/// the cover cache holds (the warm keeps every album at 300 and 600), as one
/// stream of frames in the order the ids were given. A record whose cover is
/// not kept, or an id that is neither an album's nor a track's, is answered `NOT_HELD`, and the
/// client asks `getCoverArt` for it; nothing here opens a music file or
/// decodes an image, so a library's covers cost the server file reads alone.
/// See docs/reference/protocol/server-extensions.md#koancovers.
pub(super) async fn koan_covers(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    let params = RawParams::parse(raw.as_deref());
    let auth = params.auth();
    let json = auth.wants_json();
    let ids: Vec<String> = params.all("id").map(str::to_owned).collect();
    let size = crate::covers::snap(params.get("size").and_then(|s| s.parse().ok()));
    let held = state.clone();
    let resolved = offload(move || {
        let db = authed_db(&held, &auth)?;
        if ids.is_empty() {
            return Err(SubsonicError::missing_param("id"));
        }
        if ids.len() > koan_core::remote::covers::BULK_MAX {
            return Err(SubsonicError::bad_param("id"));
        }
        held.covers.forget_evicted(&db.conn);
        // A record by its own id, or by one of its tracks': a client that
        // synced songs alone knows a record's tracks, and a track's cover is
        // its record's, as `getCoverArt` answers it.
        let kinds: Vec<(String, Option<EntityKind>, i64)> = ids
            .into_iter()
            .map(|raw| match resolve_entity(&db, &raw) {
                Ok((kind, id)) => (raw, kind, id),
                Err(_) => (raw, None, 0),
            })
            .collect();
        let tracks: Vec<i64> = kinds
            .iter()
            .filter(|(_, kind, _)| *kind == Some(EntityKind::Song))
            .map(|(_, _, id)| *id)
            .collect();
        let records = queries::album_ids_for_tracks(&db.conn, &tracks)
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
        Ok(kinds
            .into_iter()
            .map(|(raw, kind, id)| {
                let album = match kind {
                    Some(EntityKind::Album) => Some(id),
                    Some(EntityKind::Song) => records.get(&id).copied(),
                    _ => None,
                };
                (raw, album)
            })
            .collect::<Vec<_>>())
    })
    .await;
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(e) => return SubsonicResponse::error(json, &e),
    };
    // Frames are made as the body is read, each cover read on the blocking
    // pool, `COVERS_AHEAD` ahead of the one being sent so the next is ready
    // when the socket takes it: a client reading slowly holds no more than
    // those, and one that goes stops the reads.
    use futures_util::StreamExt as _;
    let frames = futures_util::stream::iter(resolved)
        .map(move |(raw, album)| {
            let state = state.clone();
            async move {
                let bytes = tokio::task::spawn_blocking(move || {
                    use koan_core::remote::covers::{BulkCover, frame};
                    match album.and_then(|id| state.covers.album_cover_kept(id, size)) {
                        Some(Some(cover)) => frame(&raw, BulkCover::Cover(&cover)),
                        Some(None) => frame(&raw, BulkCover::None),
                        None => frame(&raw, BulkCover::NotHeld),
                    }
                })
                .await
                .map_err(std::io::Error::other)?;
                Ok::<_, std::io::Error>(axum::body::Bytes::from(bytes))
            }
        })
        .buffered(COVERS_AHEAD);
    Response::builder()
        .header(header::CONTENT_TYPE, koan_core::remote::covers::BULK_TYPE)
        .body(axum::body::Body::from_stream(frames))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;
    use axum::http::Request;

    /// `getCoverArt` resolves an id by its kind: `id=5` meaning "album 5" must not
    /// serve track 5's art. Seeded so the two number spaces cross: album 2 holds
    /// track 3, so `al-2` and `mf-2` must land on different files.
    #[test]
    fn test_cover_art_id_namespacing() {
        let (state, dir) = test_state();
        let db = Database::open(state.pool.path()).unwrap();
        for (n, album) in [(1, "Album One"), (2, "Album One"), (3, "Album Two")] {
            let path = dir.path().join(format!("t{}.flac", n));
            queries::upsert_track(
                &db.conn,
                &track_meta(path.to_str().unwrap(), &format!("Song {}", n), album, n),
            )
            .unwrap();
        }

        let track3 =
            queries::track_id_by_path(&db.conn, dir.path().join("t3.flac").to_str().unwrap())
                .unwrap()
                .unwrap();
        let track2 =
            queries::track_id_by_path(&db.conn, dir.path().join("t2.flac").to_str().unwrap())
                .unwrap()
                .unwrap();
        let album_two = queries::get_track_row(&db.conn, track3)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();
        assert_eq!(album_two, track2, "test needs the id spaces to overlap");

        let first = |kind, id| {
            let groups = cover_tracks(&db, kind, id).unwrap();
            PathBuf::from(groups[0][0].path.clone().unwrap())
        };
        // `al-2` is Album Two — track 3's file, not track 2's.
        assert_eq!(
            first(Some(EntityKind::Album), album_two),
            dir.path().join("t3.flac")
        );
        // `mf-2` and a bare `2` are both track 2.
        assert_eq!(
            first(Some(EntityKind::Song), track2),
            dir.path().join("t2.flac")
        );
        assert_eq!(first(None, track2), dir.path().join("t2.flac"));
        // An artist resolves through their albums, one group each.
        let artist_id = queries::all_artists(&db.conn).unwrap()[0].id;
        assert_eq!(
            cover_tracks(&db, Some(EntityKind::Artist), artist_id)
                .unwrap()
                .len(),
            2
        );
    }

    /// A library that keeps its covers as `folder.jpg` beside the tracks, with
    /// nothing embedded, answers `getCoverArt` from the image.
    #[tokio::test]
    async fn test_cover_art_from_a_folder_image() {
        let (state, dir) = test_state();
        let album = dir.path().join("Album");
        std::fs::create_dir_all(&album).unwrap();
        let track = album.join("01.flac");
        std::fs::write(&track, b"no tags here").unwrap();
        let jpeg = {
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new(&mut out)
                .encode_image(&image::RgbImage::from_pixel(
                    64,
                    64,
                    image::Rgb([200, 30, 30]),
                ))
                .unwrap();
            out
        };
        std::fs::write(album.join("folder.jpg"), &jpeg).unwrap();
        let db = Database::open(state.pool.path()).unwrap();
        queries::upsert_track(
            &db.conn,
            &track_meta(track.to_str().unwrap(), "Song", "Album", 1),
        )
        .unwrap();
        let track_id = queries::track_id_by_path(&db.conn, track.to_str().unwrap())
            .unwrap()
            .unwrap();
        let album_id = queries::get_track_row(&db.conn, track_id)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();
        let app = build_test_router(state);
        for id in [format!("al-{album_id}"), format!("mf-{track_id}")] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/rest/getCoverArt?{}&id={id}", auth_query("")))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/jpeg");
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(&body[..], &jpeg[..], "a small JPEG is served as it is");
        }
    }

    /// `koanCovers` answers the covers it has made, in the order asked.
    #[tokio::test]
    async fn test_bulk_covers_answer_what_is_made() {
        use koan_core::remote::covers::{BulkCover, read_frame};
        let (state, dir) = test_state();
        let album = dir.path().join("Album");
        std::fs::create_dir_all(&album).unwrap();
        let track = album.join("01.flac");
        std::fs::write(&track, b"no tags here").unwrap();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&image::RgbImage::from_pixel(
                64,
                64,
                image::Rgb([200, 30, 30]),
            ))
            .unwrap();
        std::fs::write(album.join("folder.jpg"), &jpeg).unwrap();
        let db = Database::open(state.pool.path()).unwrap();
        queries::upsert_track(
            &db.conn,
            &track_meta(track.to_str().unwrap(), "Song", "Album", 1),
        )
        .unwrap();
        let track_id = queries::track_id_by_path(&db.conn, track.to_str().unwrap())
            .unwrap()
            .unwrap();
        let album_id = queries::get_track_row(&db.conn, track_id)
            .unwrap()
            .unwrap()
            .album_id
            .unwrap();
        let app = build_test_router(state);
        let bulk = |app: axum::Router| async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "/rest/koanCovers?{}&size=600&id=al-{album_id}&id=al-999999&id=mf-{track_id}",
                            auth_query("")
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.headers()[header::CONTENT_TYPE],
                koan_core::remote::covers::BULK_TYPE
            );
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let mut read = &body[..];
            let mut frames = Vec::new();
            while let Some(frame) = read_frame(&mut read).unwrap() {
                frames.push(frame);
            }
            frames
        };
        let before = bulk(app.clone()).await;
        assert_eq!(
            before.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            vec![
                format!("al-{album_id}"),
                "al-999999".to_string(),
                format!("mf-{track_id}")
            ]
        );
        assert!(before.iter().all(|(_, c)| *c == BulkCover::NotHeld));
        let made = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/rest/getCoverArt?{}&id=al-{album_id}&size=600",
                        auth_query("")
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let made = axum::body::to_bytes(made.into_body(), usize::MAX)
            .await
            .unwrap();
        let after = bulk(app).await;
        assert_eq!(after[0].1, BulkCover::Cover(made.to_vec()));
        assert_eq!(after[1].1, BulkCover::NotHeld);
        assert_eq!(
            after[2].1,
            BulkCover::Cover(made.to_vec()),
            "a track's id is answered with its record's cover, which is what the preload sends"
        );
    }

    /// A record that exists but has no art is served the ensō, at the size
    /// asked for, rather than an error.
    #[tokio::test]
    async fn test_cover_art_without_art_is_the_enso() {
        let (state, dir) = test_state();
        let track = dir.path().join("01.flac");
        std::fs::write(&track, b"no tags here").unwrap();
        let db = Database::open(state.pool.path()).unwrap();
        queries::upsert_track(
            &db.conn,
            &track_meta(track.to_str().unwrap(), "Song", "Album", 1),
        )
        .unwrap();
        let track_id = queries::track_id_by_path(&db.conn, track.to_str().unwrap())
            .unwrap()
            .unwrap();
        let resp = build_test_router(state)
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/rest/getCoverArt?{}&id=mf-{track_id}&size=200",
                        auth_query("")
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(resp.headers()[header::CACHE_CONTROL], "max-age=300");
        assert_eq!(
            resp.headers()[koan_core::remote::client::PLACEHOLDER_HEADER],
            "1"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let img = image::load_from_memory(&body).unwrap();
        // 200 asked, served at the next of the sizes clients share.
        assert_eq!((img.width(), img.height()), (300, 300));
    }

    #[tokio::test]
    async fn test_cover_art_missing_album_is_error_70() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) = get_response(
            app,
            &format!("/rest/getCoverArt?{}&id=al-9999", auth_query("")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("code=\"70\""));
    }
}

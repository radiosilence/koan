//! `search3`, which clients also use to walk the whole library.

use super::*;

/// The most `search3` returns of any one kind per request. Large enough that a
/// client walking the whole library needs few pages, small enough that one
/// response stays a few megabytes.
pub(super) const SEARCH_PAGE_MAX: u32 = 1000;

/// Whether a `search3` query asks for everything. OpenSubsonic clients send an
/// empty query, and some send a pair of quotes, to list the whole library.
pub(super) fn lists_everything(query: &str) -> bool {
    matches!(query.trim(), "" | "\"\"")
}

pub(super) async fn search3(
    State(state): State<Arc<AppState>>,
    Query(params): Query<Search3Params>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params.auth, Role::Readonly, |db, caller, b| {
            let user = caller.user_id;
            let query = params
                .query
                .as_deref()
                .ok_or_else(|| SubsonicError::missing_param("query"))?;

            let artist_count = params.artist_count.unwrap_or(20).min(SEARCH_PAGE_MAX);
            let album_count = params.album_count.unwrap_or(20).min(SEARCH_PAGE_MAX);
            let song_count = params.song_count.unwrap_or(20).min(SEARCH_PAGE_MAX);
            let artist_offset = params.artist_offset.unwrap_or(0);
            let album_offset = params.album_offset.unwrap_or(0);
            let song_offset = params.song_offset.unwrap_or(0);
            let internal =
                |e: koan_core::db::connection::DbError| SubsonicError::internal(e.to_string());

            let (artists, albums, songs): (
                Vec<(i64, String)>,
                Vec<queries::AlbumRow>,
                Vec<queries::TrackRow>,
            ) = if lists_everything(query) {
                // Every kind in id order, straight off the primary keys, so an
                // offset walk is exact: nothing added mid-walk can shift a page
                // that has already been read.
                let artists = if artist_count == 0 {
                    Vec::new()
                } else {
                    queries::list_artists(
                        &db.conn,
                        &queries::ArtistQuery {
                            order: queries::ArtistOrder::Id,
                            limit: Some(artist_count),
                            offset: artist_offset,
                            ..Default::default()
                        },
                    )
                    .map_err(internal)?
                    .into_iter()
                    .map(|a| (a.id, a.name))
                    .collect()
                };
                let albums = if album_count == 0 {
                    Vec::new()
                } else {
                    queries::list_albums(
                        &db.conn,
                        &queries::AlbumQuery {
                            order: queries::AlbumOrder::Id,
                            limit: Some(album_count),
                            offset: album_offset,
                            ..Default::default()
                        },
                    )
                    .map_err(internal)?
                };
                let songs = if song_count == 0 {
                    Vec::new()
                } else {
                    queries::tracks_page(&db.conn, song_count, song_offset).map_err(internal)?
                };
                (artists, albums, songs)
            } else {
                // The search page's own listings: names for artists and
                // records, the full-text index for songs, closest first, and
                // the closest fuzzy matches when nothing holds the query.
                use koan_core::shelves::{self, Shelf};
                let shelf = Shelf::Search(query);
                let now = shelves::now();
                let artists = if artist_count == 0 {
                    Vec::new()
                } else {
                    queries::list_artists(
                        &db.conn,
                        &queries::ArtistQuery {
                            limit: Some(artist_count),
                            offset: artist_offset,
                            without_track_counts: true,
                            ..shelf.artists(user, now)
                        },
                    )
                    .map_err(internal)?
                    .into_iter()
                    .map(|a| (a.id, a.name))
                    .collect()
                };
                let albums = if album_count == 0 {
                    Vec::new()
                } else {
                    queries::list_albums(
                        &db.conn,
                        &queries::AlbumQuery {
                            limit: Some(album_count),
                            offset: album_offset,
                            ..shelf.albums(user, now)
                        },
                    )
                    .map_err(internal)?
                };
                let songs = if song_count == 0 {
                    Vec::new()
                } else {
                    shelf
                        .tracks(user, now)
                        .page(&db.conn, song_count, song_offset)
                        .map_err(internal)?
                };
                (artists, albums, songs)
            };

            let artist_extras = artist_extras(db, user, artists.iter().map(|(id, _)| *id))?;
            let album_extras = album_extras(db, user, &albums, None)?;
            let song_extras = song_extras(db, caller, &songs)?;
            let result_node = XmlNode::new(Tag::SearchResult3)
                .list(
                    Tag::Artist,
                    artists
                        .iter()
                        .map(|(id, name)| artist_id3_node(*id, name, &artist_extras)),
                )
                .list(
                    Tag::Album,
                    albums.iter().map(|a| album_to_xml_node(a, &album_extras)),
                )
                .list(
                    Tag::Song,
                    songs.iter().map(|t| track_to_xml_node(t, &song_extras)),
                );

            Ok(b.child(result_node))
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test]
    async fn test_search3() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let app = build_test_router(state);
        let (_, body) =
            get_response(app, &format!("/rest/search3?{}&query=Test", auth_query(""))).await;
        assert!(body.contains("Test Song"));
        assert!(body.contains("Test Artist"));
    }

    /// Seven tracks over three albums, for the paging tests.
    fn seed_library(state: &AppState) -> Vec<i64> {
        let db = Database::open(state.pool.path()).unwrap();
        (0..7)
            .map(|i| {
                queries::upsert_track(
                    &db.conn,
                    &track_meta(
                        &format!("/music/{i}.flac"),
                        &format!("Song {i}"),
                        &format!("Album {}", i / 3),
                        i % 3 + 1,
                    ),
                )
                .unwrap()
            })
            .collect()
    }

    async fn search3_json(state: &Arc<AppState>, params: &str) -> serde_json::Value {
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/search3?{}&{params}", auth_query("f=json")),
        )
        .await;
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        parsed["subsonic-response"]["searchResult3"].clone()
    }

    /// An empty query lists the whole library, as OpenSubsonic clients use it
    /// to — both as `query=` and as a pair of quotes.
    #[tokio::test]
    async fn test_search3_empty_query_lists_everything_in_id_order() {
        let (state, _dir) = test_state();
        let tracks = seed_library(&state);

        for query in ["query=", "query=%22%22"] {
            let r = search3_json(
                &state,
                &format!("{query}&songCount=100&albumCount=100&artistCount=100"),
            )
            .await;
            assert_eq!(ids(&state, &r["song"]), tracks, "{query}");
            assert_eq!(ids(&state, &r["album"]).len(), 3, "{query}");
            assert_eq!(ids(&state, &r["artist"]).len(), 1, "{query}");
        }
    }

    /// Pages walked by offset cover every track exactly once.
    #[tokio::test]
    async fn test_search3_song_offset_pages_without_gaps_or_repeats() {
        let (state, _dir) = test_state();
        let tracks = seed_library(&state);

        let mut walked = Vec::new();
        for offset in (0..).step_by(3) {
            let r = search3_json(
                &state,
                &format!("query=&songCount=3&songOffset={offset}&albumCount=0&artistCount=0"),
            )
            .await;
            assert!(ids(&state, &r["album"]).is_empty() && ids(&state, &r["artist"]).is_empty());
            let page = ids(&state, &r["song"]);
            if page.is_empty() {
                break;
            }
            walked.extend(page);
        }
        assert_eq!(walked, tracks);
    }

    #[tokio::test]
    async fn test_search3_album_and_artist_offsets_are_honoured() {
        let (state, _dir) = test_state();
        seed_library(&state);

        let all = ids(
            &state,
            &search3_json(&state, "query=&albumCount=10&songCount=0").await["album"],
        );
        let second = ids(
            &state,
            &search3_json(&state, "query=&albumCount=1&albumOffset=1&songCount=0").await["album"],
        );
        assert_eq!(second, vec![all[1]]);

        let past = search3_json(&state, "query=&artistCount=5&artistOffset=1&songCount=0").await;
        assert!(ids(&state, &past["artist"]).is_empty());

        // A text search pages its songs too.
        let first = ids(
            &state,
            &search3_json(&state, "query=Song&songCount=2").await["song"],
        );
        let next = ids(
            &state,
            &search3_json(&state, "query=Song&songCount=2&songOffset=2").await["song"],
        );
        assert_eq!(first.len(), 2);
        assert_eq!(next.len(), 2);
        assert!(first.iter().all(|id| !next.contains(id)));
    }

    /// A misspelt query finds the artist, the record and its songs, as the
    /// apps' search does.
    #[tokio::test]
    async fn test_search3_typo_falls_back_to_fuzzy_matches() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let r = search3_json(&state, "query=Tst%20Artst").await;
        assert_eq!(r["artist"][0]["name"], "Test Artist", "{r}");
        assert_eq!(r["album"].as_array().map(Vec::len), Some(1), "{r}");
        assert!(r["song"][0]["title"].is_string(), "{r}");
    }

    #[tokio::test]
    async fn test_search3_song_count_is_capped() {
        let (state, _dir) = test_state();
        {
            let db = Database::open(state.pool.path()).unwrap();
            db.conn.execute_batch("BEGIN").unwrap();
            for i in 0..(SEARCH_PAGE_MAX + 5) {
                queries::upsert_track(
                    &db.conn,
                    &track_meta(&format!("/m/{i}.flac"), &format!("T{i}"), "A", 1),
                )
                .unwrap();
            }
            db.conn.execute_batch("COMMIT").unwrap();
        }
        let r = search3_json(&state, "query=&songCount=5000&albumCount=0&artistCount=0").await;
        assert_eq!(ids(&state, &r["song"]).len(), SEARCH_PAGE_MAX as usize);
    }
}

//! Browsing the library by ID3 tags and by folder: artists, albums, songs,
//! album lists, random songs, artist info and lyrics.

use super::*;

pub(super) async fn get_artists(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params, Role::Readonly, |db, user, b| {
            let index_map = artist_index(db)?;
            let extras = artist_extras(db, user, index_map.values().flatten().map(|a| a.id))?;

            let mut artists_node = XmlNode::new("artists")
                .attr("ignoredArticles", IGNORED_ARTICLES)
                .array_of("index");
            for (letter, group) in &index_map {
                let mut index_node = XmlNode::new("index")
                    .attr("name", letter)
                    .array_of("artist");
                for artist in group {
                    index_node = index_node.child(
                        artist_id3_node(artist.id, &artist.name, &extras)
                            .attr_int("albumCount", artist.album_count),
                    );
                }
                artists_node = artists_node.child(index_node);
            }

            Ok(b.child(artists_node))
        })
    })
    .await
}

/// The file-browse counterpart of `getArtists`. DSub and every folder-oriented
/// client enumerate the library through this and `getMusicDirectory`, so
/// without them they see an empty server.
#[derive(Debug, Default, Deserialize)]
pub(super) struct IndexesParams {
    #[serde(rename = "ifModifiedSince")]
    pub(super) if_modified_since: Option<i64>,
}

pub(super) async fn get_indexes(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsonicParams>,
    Query(indexes): Query<IndexesParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params, Role::Readonly, |db, user, b| {
            let last_modified = state.last_modified(db)?;
            // Nothing changed since the caller last looked: the timestamp
            // alone, which is all a client checking for changes reads.
            if indexes
                .if_modified_since
                .is_some_and(|since| since >= last_modified)
            {
                return Ok(b.child(
                    XmlNode::new("indexes")
                        .attr_int("lastModified", last_modified)
                        .attr("ignoredArticles", IGNORED_ARTICLES),
                ));
            }
            let index_map = artist_index(db)?;
            let extras = artist_extras(db, user, index_map.values().flatten().map(|a| a.id))?;

            let mut indexes_node = XmlNode::new("indexes")
                .attr_int("lastModified", last_modified)
                .attr("ignoredArticles", IGNORED_ARTICLES)
                .array_of("index");
            for (letter, group) in &index_map {
                let mut index_node = XmlNode::new("index")
                    .attr("name", letter)
                    .array_of("artist");
                for artist in group {
                    index_node = index_node.child(
                        XmlNode::new("artist")
                            .attr("id", &extras.uids.artist(artist.id))
                            .attr("name", &artist.name)
                            .attr_opt(
                                "starred",
                                extras.starred.get(&artist.id).map(|&at| iso(at)).as_deref(),
                            )
                            .attr_opt_int(
                                "userRating",
                                extras.rating.get(&artist.id).map(|&r| r.into()),
                            ),
                    );
                }
                indexes_node = indexes_node.child(index_node);
            }

            Ok(b.child(indexes_node))
        })
    })
    .await
}

/// One level of the browse tree: an artist directory lists its albums, an album
/// directory lists its songs.
pub(super) async fn get_music_directory(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params.auth, Role::Readonly, |db, caller, b| {
            let user = caller.user_id;
            let (kind, id) = require_entity(db, params.id.as_deref())?;

            // A bare row id is ambiguous — artists and albums number separately —
            // so try the artist table first and fall through. Clients that arrived
            // via `getIndexes` send a uid and never hit this.
            if kind != Some(EntityKind::Album) {
                let artist = queries::get_artist(&db.conn, id)
                    .map_err(|e| SubsonicError::internal(e.to_string()))?
                    .filter(|a| a.album_count > 0);
                if let Some(artist) = artist {
                    let albums = queries::albums_for_artist(&db.conn, id)
                        .map_err(|e| SubsonicError::internal(e.to_string()))?;
                    let extras = album_extras(db, user, &albums, None)?;
                    let uids = &extras.uids;
                    let mut dir = XmlNode::new("directory")
                        .attr("id", &uids.artist(artist.id))
                        .attr("name", &artist.name)
                        .array_of("child");
                    for album in &albums {
                        dir = dir.child(album_child_node(album, &extras));
                    }
                    return Ok(b.child(dir));
                }
            }

            let album = queries::get_album(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Directory"))?;
            let tracks = queries::tracks_for_album(&db.conn, id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;

            let uids = Uids::load(db, [album.artist_id], [album.id], [])?;
            let mut dir = XmlNode::new("directory")
                .attr("id", &uids.album(album.id))
                .attr("parent", &uids.artist(album.artist_id))
                .attr("name", &album.title)
                .array_of("child");
            let extras = song_extras(db, caller, &tracks)?;
            for track in &tracks {
                dir = dir.child(track_node(track, "child", &extras));
            }
            Ok(b.child(dir))
        })
    })
    .await
}

pub(super) async fn get_artist(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            let artist_id = require_id(db, params.id.as_deref(), EntityKind::Artist)?;

            let artist = queries::get_artist(&db.conn, artist_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Artist"))?;

            let albums = queries::albums_for_artist(&db.conn, artist_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;

            let artists = artist_extras(db, user, [artist.id])?;
            let extras = album_extras(db, user, &albums, None)?;
            Ok(b.child(
                artist_id3_node(artist.id, &artist.name, &artists)
                    .attr_int("albumCount", albums.len() as i64)
                    .list(
                        "album",
                        albums.iter().map(|album| album_to_xml_node(album, &extras)),
                    ),
            ))
        })
    })
    .await
}

/// An artist's biography and photograph, resolved by MusicBrainz id
/// (`koan_core::artist_info`): from what the walk already holds, or looked up
/// now. Beside Subsonic's fields, koan's (`koanArtistInfo`): the article the
/// biography opens, the photograph's credit, and whether `koanArtistImage`
/// has it. A lookup that failed is an error, so a client resolves the artist
/// itself rather than remembering a miss; one that found nothing is an
/// empty answer.
pub(super) async fn get_artist_info2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond(&state, &params.auth, |_, b| {
            let (artist_id, mbid) = {
                let db = state.open_db()?;
                let artist_id = require_id(&db, params.id.as_deref(), EntityKind::Artist)?;
                let mbid = queries::artist_mbid(&db.conn, artist_id)?
                    .ok_or_else(|| SubsonicError::not_found("Artist"))?;
                (artist_id, mbid)
            };
            // No connection is held while MusicBrainz and Wikimedia answer.
            let info = koan_core::artist_info::fetch_by_id(&state.pool, artist_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .unwrap_or_default();
            Ok(b.child(artist_info_node(&info, mbid.as_deref())))
        })
    })
    .await
}

pub(super) fn artist_info_node(
    info: &koan_core::artist_info::ArtistInfo,
    mbid: Option<&str>,
) -> XmlNode {
    let text = |tag: &str, value: &str| XmlNode::scalar(tag, AttrValue::Str(value.into()));
    let mut node = XmlNode::new("artistInfo2");
    if let Some(bio) = &info.bio {
        node = node.child(text("biography", bio));
    }
    if let Some(mbid) = mbid {
        node = node.child(text("musicBrainzId", mbid));
    }
    let image = info.image_url.as_deref().filter(|u| !u.is_empty());
    if let Some(url) = image {
        for tag in ["smallImageUrl", "mediumImageUrl", "largeImageUrl"] {
            node = node.child(text(tag, url));
        }
    }
    if let Some(url) = &info.bio_url {
        node = node.child(text("koanBioUrl", url));
    }
    if let Some(credit) = image.and(info.image_credit.as_deref()) {
        node = node.child(text("koanImageCredit", credit));
    }
    node.child(XmlNode::scalar(
        "koanImage",
        AttrValue::Bool(image.is_some()),
    ))
}

/// `koanArtistImage`: the artist's photograph as this server keeps it,
/// downloaded and kept first where the walk has not reached the artist yet.
/// Not found (70) when the artist has none.
pub(super) async fn koan_artist_image(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        let json = params.auth.wants_json();
        let answer = (|| {
            let artist_id = {
                let db = authed_db(&state, &params.auth)?;
                require_id(&db, params.id.as_deref(), EntityKind::Artist)?
            };
            koan_core::artist_info::image(&state.pool, artist_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Artist image"))
        })();
        match answer {
            Ok(bytes) => (
                StatusCode::OK,
                [
                    (
                        header::CONTENT_TYPE,
                        koan_core::index::folder_art::image_mime(&bytes).unwrap_or("image/jpeg"),
                    ),
                    (header::CACHE_CONTROL, "max-age=86400"),
                ],
                bytes,
            )
                .into_response(),
            Err(e) => SubsonicResponse::error(json, &e),
        }
    })
    .await
}

pub(super) async fn get_album(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params.auth, Role::Readonly, |db, caller, b| {
            let user = caller.user_id;
            let album_id = require_id(db, params.id.as_deref(), EntityKind::Album)?;

            let album = queries::get_album(&db.conn, album_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Album"))?;

            let tracks = queries::tracks_for_album(&db.conn, album_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let albums = album_extras(db, user, [&album], Some(&tracks))?;
            let songs = song_extras(db, caller, &tracks)?;
            Ok(b.child(
                album_to_xml_node(&album, &albums)
                    .list("song", tracks.iter().map(|t| track_to_xml_node(t, &songs))),
            ))
        })
    })
    .await
}

/// Albums ordered by `type`, paged in SQL. Shared by `getAlbumList` and
/// `getAlbumList2`, which differ only in the element they hang the list off.
pub(super) fn album_list(
    db: &Database,
    user: i64,
    params: &AlbumListParams,
    tag: &str,
) -> Result<XmlNode, SubsonicError> {
    let limit = params.size.unwrap_or(20).clamp(0, 500) as u32;
    let offset = params.offset.unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
    let played = |order| {
        queries::played_albums(&db.conn, user, order, limit, offset)
            .map_err(|e| SubsonicError::internal(e.to_string()))
    };
    let page = match params.list_type.as_deref().unwrap_or("alphabeticalByName") {
        "recent" => played(queries::PlayedOrder::Recent)?,
        "frequent" => played(queries::PlayedOrder::Frequent)?,
        "highest" => queries::highest_rated_albums(&db.conn, user, limit, offset)
            .map_err(|e| SubsonicError::internal(e.to_string()))?,
        list_type => {
            let q = queries::AlbumQuery {
                limit: Some(limit),
                offset,
                ..album_query(list_type, user, params)?
            };
            queries::list_albums(&db.conn, &q)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
        }
    };
    let extras = album_extras(db, user, &page, None)?;
    Ok(XmlNode::new(tag).list(
        "album",
        page.iter().map(|album| album_to_xml_node(album, &extras)),
    ))
}

/// The library listing a `getAlbumList` type asks for, unpaged.
pub(super) fn album_query<'a>(
    list_type: &str,
    user: i64,
    params: &'a AlbumListParams,
) -> Result<queries::AlbumQuery<'a>, SubsonicError> {
    use queries::{AlbumFilter, AlbumOrder, AlbumQuery};
    let order = |order| AlbumQuery {
        order,
        ..Default::default()
    };
    Ok(match list_type {
        "alphabeticalByName" => order(AlbumOrder::Title),
        "alphabeticalByArtist" => order(AlbumOrder::ArtistThenDate),
        // Recently added, as Subsonic means it — not release date.
        "newest" => order(AlbumOrder::RecentlyAdded),
        "random" => order(AlbumOrder::Random(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64,
        )),
        "starred" => AlbumQuery {
            favourites_of: Some(user),
            ..Default::default()
        },
        "byGenre" => AlbumQuery {
            filter: AlbumFilter {
                genre: Some(
                    params
                        .genre
                        .as_deref()
                        .ok_or_else(|| SubsonicError::missing_param("genre"))?,
                ),
                ..Default::default()
            },
            ..Default::default()
        },
        "byYear" => {
            let from = params
                .from_year
                .ok_or_else(|| SubsonicError::missing_param("fromYear"))?;
            let to = params
                .to_year
                .ok_or_else(|| SubsonicError::missing_param("toYear"))?;
            AlbumQuery {
                filter: AlbumFilter {
                    year_from: Some(from.min(to)),
                    year_to: Some(from.max(to)),
                    ..Default::default()
                },
                // fromYear after toYear asks for the range newest first.
                ..order(if from > to {
                    AlbumOrder::YearDesc
                } else {
                    AlbumOrder::Date
                })
            }
        }
        _ => return Err(SubsonicError::bad_param("type")),
    })
}

pub(super) async fn get_album_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlbumListParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            Ok(b.child(album_list(db, user, &params, "albumList")?))
        })
    })
    .await
}

pub(super) async fn get_album_list2(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlbumListParams>,
) -> Response {
    offload_response(move || {
        respond_db_user(&state, &params.auth, Role::Readonly, |db, user, b| {
            Ok(b.child(album_list(db, user, &params, "albumList2")?))
        })
    })
    .await
}

pub(super) async fn get_song(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params.auth, Role::Readonly, |db, caller, b| {
            let track_id = require_id(db, params.id.as_deref(), EntityKind::Song)?;
            let track = queries::get_track_row(&db.conn, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Song"))?;
            let extras = song_extras(db, caller, [&track])?;
            Ok(b.child(track_to_xml_node(&track, &extras)))
        })
    })
    .await
}

/// The lyrics koan has cached for a song, as one `structuredLyrics` entry, or
/// none. Only the cache is read: fetching from LRCLIB is the player's job, and
/// not something to do inside a client's request.
pub(super) async fn get_lyrics_by_song_id(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IdParam>,
) -> Response {
    offload_response(move || {
        respond_db(&state, &params.auth, |db, b| {
            let track_id = require_id(db, params.id.as_deref(), EntityKind::Song)?;
            let track = queries::get_track_row(&db.conn, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?
                .ok_or_else(|| SubsonicError::not_found("Song"))?;
            let cached = queries::get_cached_lyrics(&db.conn, track_id)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let entries = cached.map(|c| structured_lyrics(&track, &c.content, c.synced));
            Ok(b.child(XmlNode::new("lyricsList").list("structuredLyrics", entries)))
        })
    })
    .await
}

/// A lyrics text as `structuredLyrics`. Synced lyrics are LRC, whose lines
/// carry their start time; plain lyrics are one `line` per line of text.
pub(super) fn structured_lyrics(track: &queries::TrackRow, content: &str, synced: bool) -> XmlNode {
    let lines: Vec<XmlNode> = if synced {
        koan_core::lyrics::parse_lrc(content)
            .into_iter()
            .map(|l| {
                XmlNode::new("line")
                    .attr_int("start", (l.time_secs * 1000.0).round() as i64)
                    .text(&l.text)
            })
            .collect()
    } else {
        content
            .lines()
            .map(|l| XmlNode::new("line").text(l.trim_end()))
            .collect()
    };
    XmlNode::new("structuredLyrics")
        .attr("displayArtist", &track.artist_name)
        .attr("displayTitle", &track.title)
        // LRCLIB does not say which language a text is in.
        .attr("lang", "und")
        .attr_bool("synced", synced)
        .list("line", lines)
}

pub(super) async fn get_random_songs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RandomSongsParams>,
) -> Response {
    offload_response(move || {
        respond_db_caller(&state, &params.auth, Role::Readonly, |db, caller, b| {
            // Capped as getAlbumList is: every song is built in memory.
            let size = params.size.unwrap_or(10).min(500);
            let filter = queries::RandomFilter {
                genre: params.genre.as_deref(),
                year_from: params.from_year,
                year_to: params.to_year,
                ..Default::default()
            };
            let tracks = queries::random_tracks_where(&db.conn, size, &filter)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            let extras = song_extras(db, caller, &tracks)?;
            Ok(b.child(
                XmlNode::new("randomSongs")
                    .list("song", tracks.iter().map(|t| track_to_xml_node(t, &extras))),
            ))
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[tokio::test]
    async fn test_get_artists_empty() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getArtists?{}", auth_query(""))).await;
        assert!(body.contains("status=\"ok\""));
        assert!(body.contains("<artists"));
    }

    #[tokio::test]
    async fn test_get_artists_with_data() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let app = build_test_router(state);
        let (_, body) = get_response(app, &format!("/rest/getArtists?{}", auth_query(""))).await;
        assert!(body.contains("Test Artist"));
    }

    #[tokio::test]
    async fn test_get_artist_by_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let artists = queries::all_artists(&db.conn).unwrap();
        let artist = &artists[0];

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getArtist?{}&id={}", auth_query(""), artist.id),
        )
        .await;
        assert!(body.contains("Test Artist"));
        assert!(body.contains("Test Album"));
    }

    #[tokio::test]
    async fn test_get_album_by_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let albums = queries::all_albums(&db.conn).unwrap();
        let album = &albums[0];

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getAlbum?{}&id={}", auth_query(""), album.id),
        )
        .await;
        assert!(body.contains("Test Album"));
        assert!(body.contains("Test Song"));
    }

    #[tokio::test]
    async fn test_get_song_by_id() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let albums = queries::all_albums(&db.conn).unwrap();
        let tracks = queries::tracks_for_album(&db.conn, albums[0].id).unwrap();
        let track = &tracks[0];

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getSong?{}&id={}", auth_query(""), track.id),
        )
        .await;
        assert!(body.contains("Test Song"));
        assert!(body.contains("Test Artist"));
    }

    /// A song's `path` describes the server's filesystem: an admin is told
    /// it, anyone else is not.
    #[tokio::test]
    async fn song_paths_go_to_admins_only() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let track = queries::all_tracks(&db.conn).unwrap()[0].id;
        let app = build_test_router(state);
        let song = |auth: &str| {
            let app = app.clone();
            let uri = format!("/rest/getSong?{auth}&v=1.16.1&c=test&id={track}");
            async move { get_response(app, &uri).await.1 }
        };

        let admin = song("u=owner&p=sesame").await;
        assert!(admin.contains(r#"path="/music/test.flac""#), "{admin}");
        for other in ["u=mate&p=hunter22", &auth_query("")] {
            let body = song(other).await;
            assert!(body.contains("Test Song"), "{body}");
            assert!(!body.contains("test.flac"), "{body}");
        }
    }

    #[tokio::test]
    async fn test_get_album_list2() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getAlbumList2?{}&type=alphabeticalByName&size=10",
                auth_query("")
            ),
        )
        .await;
        assert!(body.contains("Test Album"));
    }

    #[tokio::test]
    async fn test_get_album_list_v1() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getAlbumList?{}&type=newest", auth_query("")),
        )
        .await;
        assert!(body.contains("<albumList"));
        assert!(body.contains("Test Album"));
    }

    #[tokio::test]
    async fn album_lists_answer_every_type_from_sql() {
        let (state, _dir) = test_state();
        let [alpha, beta, gamma] = seed_shelves(&state)[..] else {
            unreachable!()
        };

        assert_eq!(
            album_list2(&state, "type=byGenre&genre=JAZZ").await,
            ["Alpha", "Gamma"]
        );
        assert_eq!(
            album_list2(&state, "type=byYear&fromYear=1990&toYear=2010").await,
            ["Beta", "Gamma"]
        );
        assert_eq!(
            album_list2(&state, "type=byYear&fromYear=2010&toYear=1990").await,
            ["Gamma", "Beta"],
            "a reversed range lists newest first"
        );
        assert_eq!(
            album_list2(&state, "type=alphabeticalByName&size=2&offset=1").await,
            ["Beta", "Gamma"]
        );
        assert_eq!(album_list2(&state, "type=random").await.len(), 3);

        assert!(album_list2(&state, "type=starred").await.is_empty());
        let beta_album = uid_of(
            &state,
            queries::UidKind::Album,
            queries::get_track_row(&Database::open(state.pool.path()).unwrap().conn, beta)
                .unwrap()
                .unwrap()
                .album_id
                .unwrap(),
        );
        get_response(
            build_test_router(state.clone()),
            &format!("/rest/star?{}&albumId={beta_album}", auth_query("")),
        )
        .await;
        assert_eq!(album_list2(&state, "type=starred").await, ["Beta"]);

        // Two plays of Alpha, the later of Gamma's one in between, sent as one
        // batched scrobble.
        let (_, body) = get_response(
            build_test_router(state.clone()),
            &format!(
                "/rest/scrobble?{}&id={alpha}&time=1000000&id={gamma}&time=3000000&id={alpha}&time=2000000",
                auth_query("")
            ),
        )
        .await;
        assert!(body.contains("status=\"ok\""), "{body}");
        assert_eq!(album_list2(&state, "type=recent").await, ["Gamma", "Alpha"]);
        assert_eq!(
            album_list2(&state, "type=frequent").await,
            ["Alpha", "Gamma"]
        );
        assert!(album_list2(&state, "type=highest").await.is_empty());

        let v = json_of(
            build_test_router(state.clone()),
            &format!("/rest/getAlbumList2?{}&type=byGenre", auth_query("f=json")),
        )
        .await;
        assert_eq!(v["error"]["code"], 10, "byGenre needs a genre");
        let v = json_of(
            build_test_router(state),
            &format!("/rest/getAlbumList2?{}&type=nonsense", auth_query("f=json")),
        )
        .await;
        assert_eq!(v["error"]["code"], 10, "an unknown type is refused");
    }

    #[tokio::test]
    async fn artist_info_is_answered_from_what_the_server_holds() {
        let (state, _dir) = test_state();
        let (found, missed) = {
            let db = Database::open(state.pool.path()).unwrap();
            let artist = |name: &str| {
                let id = queries::get_or_create_artist(&db.conn, name, None).unwrap();
                db.conn
                    .execute(
                        "UPDATE artists SET mbid = 'mb-' || id WHERE id = ?1",
                        rusqlite::params![id],
                    )
                    .unwrap();
                id
            };
            let (found, missed) = (artist("Glass Candy"), artist("Desire"));
            let now = koan_core::clock::unix_secs();
            db.conn
                .execute(
                    "INSERT INTO artist_info (artist_id, bio, bio_url, image_url, image_credit, fetched_at)
                     VALUES (?1, 'An American duo.', 'https://en.wikipedia.org/wiki/Glass_Candy',
                             'https://upload.wikimedia.org/glass.jpg', 'A. Photographer, CC BY-SA 4.0', ?3),
                            (?2, NULL, NULL, NULL, NULL, ?3)",
                    rusqlite::params![found, missed, now],
                )
                .unwrap();
            (found, missed)
        };
        let (found_uid, missed_uid) = (
            uid_of(&state, queries::UidKind::Artist, found),
            uid_of(&state, queries::UidKind::Artist, missed),
        );
        let get = |path: String| {
            let state = state.clone();
            async move {
                json_of(
                    build_test_router(state),
                    &format!("/rest/{path}&{}", auth_query("f=json")),
                )
                .await
            }
        };

        let v = get(format!("getArtistInfo2?id={found_uid}")).await;
        let info = &v["artistInfo2"];
        assert_eq!(info["biography"], "An American duo.", "{v}");
        assert_eq!(info["musicBrainzId"], format!("mb-{found}"));
        assert_eq!(
            info["largeImageUrl"],
            "https://upload.wikimedia.org/glass.jpg"
        );
        assert_eq!(
            info["koanBioUrl"],
            "https://en.wikipedia.org/wiki/Glass_Candy"
        );
        assert_eq!(info["koanImageCredit"], "A. Photographer, CC BY-SA 4.0");
        assert_eq!(info["koanImage"], true);

        let v = get(format!("getArtistInfo2?id={missed_uid}")).await;
        let info = &v["artistInfo2"];
        assert_eq!(v["status"], "ok", "{v}");
        assert!(info["biography"].is_null(), "{v}");
        assert_eq!(info["koanImage"], false);

        let v = get(format!("koanArtistImage?id={missed_uid}")).await;
        assert_eq!(v["error"]["code"], 70, "{v}");
    }

    /// Folder-browse clients read artists from `getIndexes` and albums from
    /// `getMusicDirectory`, and get the same marks as ID3 clients there.
    #[tokio::test]
    async fn folder_browse_carries_starred_and_ratings_in_json_and_xml() {
        let (state, _dir) = test_state();
        let [alpha, ..] = seed_shelves(&state)[..] else {
            unreachable!()
        };
        let other = {
            let db = Database::open(state.pool.path()).unwrap();
            let mut meta = track_meta("/music/other.flac", "Other", "Other", 1);
            meta.artist = "Other Artist".into();
            meta.album_artist = Some("Other Artist".into());
            queries::upsert_track(&db.conn, &meta).unwrap()
        };
        let uids = |track| {
            let db = Database::open(state.pool.path()).unwrap();
            let row = queries::get_track_row(&db.conn, track).unwrap().unwrap();
            (
                uid_of(&state, queries::UidKind::Track, track),
                uid_of(&state, queries::UidKind::Album, row.album_id.unwrap()),
                uid_of(&state, queries::UidKind::Artist, row.artist_id.unwrap()),
            )
        };
        let (song, album, artist) = uids(alpha);
        let (_, _, other_artist) = uids(other);
        assert_ne!(artist, other_artist);
        let call = |path: String| {
            let state = state.clone();
            async move { json_of(build_test_router(state), &path).await }
        };
        call(format!(
            "/rest/star?{}&id={song}&albumId={album}&artistId={artist}",
            auth_query("f=json")
        ))
        .await;
        call(format!(
            "/rest/setRating?{}&id={album}&rating=4",
            auth_query("f=json")
        ))
        .await;
        call(format!(
            "/rest/setRating?{}&id={artist}&rating=3",
            auth_query("f=json")
        ))
        .await;
        let get = |path: String| call(format!("/rest/{path}&{}", auth_query("f=json")));
        let is_iso = |v: &serde_json::Value| {
            v.as_str()
                .is_some_and(|s| s.len() == 24 && s.contains('T') && s.ends_with('Z'))
        };
        let indexed = |v: &serde_json::Value, id: &str| {
            v["indexes"]["index"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|i| i["artist"].as_array().unwrap().clone())
                .find(|a| a["id"] == id)
                .unwrap_or_else(|| panic!("{id} not indexed: {v}"))
        };

        let v = get("getIndexes?".to_owned()).await;
        let starred = indexed(&v, &artist);
        assert!(is_iso(&starred["starred"]), "{v}");
        assert_eq!(starred["userRating"], 3, "{v}");
        let other = indexed(&v, &other_artist);
        assert!(
            other["starred"].is_null() && other["userRating"].is_null(),
            "{v}"
        );

        let v = get(format!("getMusicDirectory?id={artist}")).await;
        let child = &v["directory"]["child"][0];
        assert!(is_iso(&child["starred"]), "{v}");
        assert_eq!(child["userRating"], 4, "{v}");
        let v = get(format!("getMusicDirectory?id={album}")).await;
        assert!(is_iso(&v["directory"]["child"][0]["starred"]), "{v}");

        let mate = |path: &str| {
            call(format!(
                "/rest/{path}&u=mate&p=hunter22&v=1.16.1&c=test&f=json"
            ))
        };
        let v = mate("getIndexes?").await;
        assert!(
            indexed(&v, &artist)["starred"].is_null(),
            "another account's: {v}"
        );
        let v = mate(&format!("getMusicDirectory?id={artist}")).await;
        assert!(v["directory"]["child"][0]["starred"].is_null(), "{v}");

        let xml = |path: String| {
            let state = state.clone();
            async move {
                get_response(
                    build_test_router(state),
                    &format!("/rest/{path}&{}", auth_query("")),
                )
                .await
                .1
            }
        };
        let body = xml("getIndexes?".to_owned()).await;
        assert_eq!(body.matches(" starred=\"").count(), 1, "{body}");
        assert_eq!(body.matches(" userRating=\"3\"").count(), 1, "{body}");
        let body = xml(format!("getMusicDirectory?id={artist}")).await;
        assert_eq!(body.matches(" starred=\"").count(), 1, "{body}");
        assert_eq!(body.matches(" userRating=\"4\"").count(), 1, "{body}");
    }

    #[tokio::test]
    async fn random_songs_filter_in_sql() {
        let (state, _dir) = test_state();
        seed_shelves(&state);
        let random = |params: &'static str| {
            let state = state.clone();
            async move {
                let v = json_of(
                    build_test_router(state),
                    &format!("/rest/getRandomSongs?{}&{params}", auth_query("f=json")),
                )
                .await;
                let mut got = names(&v["randomSongs"]["song"]);
                got.sort();
                got
            }
        };
        // A genre that is a small share of the library still fills the draw.
        assert_eq!(random("genre=jazz&size=10").await, ["Alpha", "Gamma"]);
        assert_eq!(random("fromYear=1990&toYear=1999").await, ["Beta"]);
        assert_eq!(random("size=4000000000").await.len(), 3);
    }

    #[tokio::test]
    async fn test_get_song_not_found() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (_, body) =
            get_response(app, &format!("/rest/getSong?{}&id=99999", auth_query(""))).await;
        assert!(body.contains("status=\"failed\""));
        assert!(body.contains("code=\"70\""));
    }

    /// A client keeping a copy of the library asks whether it moved; a
    /// deletion has to move it, though it touches no file.
    #[tokio::test]
    async fn get_indexes_says_when_the_library_moved() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let modified = |body: &str| -> i64 {
            let at = body.find("lastModified=\"").unwrap() + "lastModified=\"".len();
            body[at..].split('"').next().unwrap().parse().unwrap()
        };

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getIndexes?{}", auth_query(""))).await;
        let first = modified(&body);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getIndexes?{}&ifModifiedSince={first}",
                auth_query("")
            ),
        )
        .await;
        assert_eq!(modified(&body), first);
        assert!(!body.contains("<artist"), "unchanged: the timestamp alone");

        std::thread::sleep(std::time::Duration::from_millis(5));
        let db = Database::open(state.pool.path()).unwrap();
        db.conn
            .execute(
                "DELETE FROM tracks WHERE id = (SELECT MAX(id) FROM tracks)",
                [],
            )
            .unwrap();

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getIndexes?{}&ifModifiedSince={first}",
                auth_query("")
            ),
        )
        .await;
        assert!(modified(&body) > first, "a deletion moves it");
    }

    #[tokio::test]
    async fn test_get_indexes_and_music_directory() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let artist_id = queries::all_artists(&db.conn).unwrap()[0].id;
        let album_id = queries::all_albums(&db.conn).unwrap()[0].id;

        let artist = uid_of(&state, queries::UidKind::Artist, artist_id);
        let album = uid_of(&state, queries::UidKind::Album, album_id);

        let app = build_test_router(state.clone());
        let (_, body) = get_response(app, &format!("/rest/getIndexes?{}", auth_query(""))).await;
        assert!(body.contains("<indexes"));
        assert!(body.contains(&format!("id=\"{artist}\"")));

        // Artist directory lists albums.
        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getMusicDirectory?{}&id={artist}", auth_query("")),
        )
        .await;
        assert!(body.contains(&format!("id=\"{album}\"")));
        assert!(body.contains("isDir=\"true\""));

        // Album directory lists songs, by the prefixed row id clients from
        // before uids hold.
        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!(
                "/rest/getMusicDirectory?{}&id=al-{}",
                auth_query(""),
                album_id
            ),
        )
        .await;
        assert!(body.contains("Test Song"));
        assert!(body.contains("<child"));
    }

    #[tokio::test]
    async fn test_get_random_songs() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getRandomSongs?{}&size=5", auth_query("")),
        )
        .await;
        assert!(body.contains("randomSongs"));
    }

    #[tokio::test]
    async fn test_lyrics_by_song_id() {
        let (state, _dir) = test_state();
        seed_data(&state);
        let db = Database::open(state.pool.path()).unwrap();
        let id = queries::all_tracks(&db.conn).unwrap()[0].id;
        let uri = format!("/rest/getLyricsBySongId?{}&id={id}", auth_query("f=json"));

        let r = json_of(build_test_router(state.clone()), &uri).await;
        assert_eq!(r["lyricsList"]["structuredLyrics"], serde_json::json!([]));

        queries::cache_lyrics(
            &db.conn,
            id,
            "lrclib",
            true,
            "[ar:Test Artist]\n[00:12.34]First\n[01:00.00]Second",
        )
        .unwrap();
        let r = json_of(build_test_router(state.clone()), &uri).await;
        let entry = &r["lyricsList"]["structuredLyrics"][0];
        assert_eq!(entry["synced"], true);
        assert_eq!(entry["lang"], "und");
        assert_eq!(entry["displayTitle"], "Test Song");
        assert_eq!(
            entry["line"],
            serde_json::json!([
                {"start": 12340, "value": "First"},
                {"start": 60000, "value": "Second"},
            ])
        );

        queries::cache_lyrics(&db.conn, id, "lrclib", false, "One\nTwo").unwrap();
        let r = json_of(build_test_router(state), &uri).await;
        let entry = &r["lyricsList"]["structuredLyrics"][0];
        assert_eq!(entry["synced"], false);
        assert_eq!(
            entry["line"],
            serde_json::json!([{"value": "One"}, {"value": "Two"}])
        );
    }
}

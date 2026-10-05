use std::sync::Arc;

use async_graphql::{Context, Object};
use koan_core::audio;
use koan_core::audio::viz::VizSnapshot;
use koan_core::config::Config;
use koan_core::db::queries;
use koan_core::db::queries::UidKind;
use koan_core::db::queries::batch::{TrackFilter, TrackOrder};
use koan_core::player::state::SharedPlayerState;

use super::helpers::{MAX_PAGE, album_year, page_offset, page_size, paginate, paginate_window};
use super::jobs::JobRegistry;
use super::types::*;
use super::{blocking, with_db};

/// What `fuzzySearch` matches against, kept between requests.
///
/// The server's library is written by scans, syncs and other processes, none
/// of which it can count, so a corpus is kept for as long as the library's
/// fingerprint holds and five minutes at most: a row added or removed is seen
/// at once, one edited in place within five minutes.
#[derive(Default)]
pub(super) struct Fuzzy(queries::CorpusCache);

impl Fuzzy {
    const MAX_AGE_SECS: u64 = 300;

    fn corpus(
        &self,
        conn: &rusqlite::Connection,
        kind: queries::CorpusKind,
    ) -> Result<queries::Corpus, koan_core::db::connection::DbError> {
        let window = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / Self::MAX_AGE_SECS;
        let version = queries::library_fingerprint(conn)? ^ window;
        self.0.get(conn, kind, version)
    }
}

// ---------------------------------------------------------------------------
// Query root
// ---------------------------------------------------------------------------

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    #[allow(clippy::too_many_arguments)]
    async fn artists(
        &self,
        ctx: &Context<'_>,
        ids: Option<Vec<async_graphql::ID>>,
        search: Option<String>,
        genre: Option<String>,
        #[graphql(default = false)] favourites_only: bool,
        after: Option<String>,
        first: Option<i32>,
        #[graphql(default_with = "ArtistSortField::Name")] sort_by: ArtistSortField,
        #[graphql(default_with = "SortDirection::Asc")] sort_dir: SortDirection,
    ) -> async_graphql::Result<Conn<GqlArtist>> {
        let ids = super::opt_row_ids(ctx, UidKind::Artist, ids.as_deref()).await?;
        let user = super::user_id(ctx);
        let rows = with_db(ctx, move |db| {
            // Only the named artists are read, when they are named.
            let mut artists = queries::list_artists(
                &db.conn,
                &queries::ArtistQuery {
                    ids: ids.as_deref(),
                    search: search.as_deref(),
                    ..Default::default()
                },
            )
            .map_err(|e| super::internal_error("db", e))?;

            if let Some(ref g) = genre {
                let g_lower = g.to_lowercase();
                let artist_ids: Vec<i64> = artists.iter().map(|a| a.id).collect();
                let genre_map = queries::genres_by_artist_ids(&db.conn, &artist_ids)
                    .map_err(|e| super::internal_error("db", e))?;
                artists.retain(|a| {
                    genre_map
                        .get(&a.id)
                        .is_some_and(|genres| genres.iter().any(|ag| ag.contains(&g_lower)))
                });
            }

            if favourites_only {
                let fav_ids = queries::favourite_artist_ids_batch(&db.conn, user)
                    .map_err(|e| super::internal_error("db", e))?;
                artists.retain(|a| fav_ids.contains(&a.id));
            }

            match sort_by {
                ArtistSortField::Name => artists.sort_by(|a, b| a.name.cmp(&b.name)),
                ArtistSortField::AlbumCount | ArtistSortField::TrackCount => {
                    let ids: Vec<i64> = artists.iter().map(|a| a.id).collect();
                    let stats = queries::batch::artist_stats(&db.conn, &ids)
                        .map_err(|e| super::internal_error("db", e))?;
                    let key = |id: i64| {
                        let s = stats.get(&id).copied().unwrap_or_default();
                        if sort_by == ArtistSortField::AlbumCount {
                            s.album_count
                        } else {
                            s.track_count
                        }
                    };
                    artists.sort_by(|a, b| key(a.id).cmp(&key(b.id)).then(a.name.cmp(&b.name)));
                }
            }
            if sort_dir == SortDirection::Desc {
                artists.reverse();
            }

            Ok(artists)
        })
        .await?;

        paginate(
            rows.into_iter().map(|row| GqlArtist { row }).collect(),
            after,
            first,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn albums(
        &self,
        ctx: &Context<'_>,
        ids: Option<Vec<async_graphql::ID>>,
        artist_id: Option<async_graphql::ID>,
        artist_ids: Option<Vec<async_graphql::ID>>,
        search: Option<String>,
        title: Option<String>,
        year_start: Option<i32>,
        year_end: Option<i32>,
        codec: Option<String>,
        label: Option<String>,
        genre: Option<String>,
        #[graphql(default = false)] favourites_only: bool,
        after: Option<String>,
        first: Option<i32>,
        #[graphql(default_with = "AlbumSortField::ArtistThenDate")] sort_by: AlbumSortField,
        #[graphql(default_with = "SortDirection::Asc")] sort_dir: SortDirection,
    ) -> async_graphql::Result<Conn<GqlAlbum>> {
        let ids = super::opt_row_ids(ctx, UidKind::Album, ids.as_deref()).await?;
        let artist_id = super::opt_row_id(ctx, UidKind::Artist, artist_id.as_ref()).await?;
        let artist_ids = super::opt_row_ids(ctx, UidKind::Artist, artist_ids.as_deref()).await?;
        let user = super::user_id(ctx);
        let rows = with_db(ctx, move |db| {
            let mut albums = if ids.is_some() && artist_ids.is_none() {
                // Only the named albums are read, when they are named.
                queries::list_albums(
                    &db.conn,
                    &queries::AlbumQuery {
                        ids: ids.as_deref(),
                        artist_id,
                        search: search.as_deref(),
                        ..Default::default()
                    },
                )
                .map_err(|e| super::internal_error("db", e))?
            } else if let Some(aid) = artist_id {
                queries::albums_for_artist(&db.conn, aid)
                    .map_err(|e| super::internal_error("db", e))?
            } else if let Some(ref aids) = artist_ids {
                queries::batch::albums_for_artists(&db.conn, aids)
                    .map_err(|e| super::internal_error("db", e))?
                    .into_values()
                    .flatten()
                    .collect()
            } else if let Some(ref query) = search {
                // Narrowed in SQL rather than over every album in the library —
                // the same core helper the native client's filter runs through.
                queries::find_albums(&db.conn, query).map_err(|e| super::internal_error("db", e))?
            } else {
                queries::all_albums(&db.conn).map_err(|e| super::internal_error("db", e))?
            };

            if let Some(ref id_list) = ids {
                albums.retain(|a| id_list.contains(&a.id));
            }

            if let Some(ref query) = search {
                let q = query.to_lowercase();
                albums.retain(|a| {
                    a.title.to_lowercase().contains(&q) || a.artist_name.to_lowercase().contains(&q)
                });
            }

            if let Some(ref t) = title {
                let t_lower = t.to_lowercase();
                albums.retain(|a| a.title.to_lowercase().contains(&t_lower));
            }

            if let Some(ys) = year_start {
                albums.retain(|a| album_year(a).map(|y| y >= ys).unwrap_or(false));
            }

            if let Some(ye) = year_end {
                albums.retain(|a| album_year(a).map(|y| y <= ye).unwrap_or(false));
            }

            if let Some(ref c) = codec {
                let c_lower = c.to_lowercase();
                albums.retain(|a| {
                    a.codec
                        .as_ref()
                        .map(|ac| ac.to_lowercase().contains(&c_lower))
                        .unwrap_or(false)
                });
            }

            if let Some(ref l) = label {
                let l_lower = l.to_lowercase();
                albums.retain(|a| {
                    a.label
                        .as_ref()
                        .map(|al| al.to_lowercase().contains(&l_lower))
                        .unwrap_or(false)
                });
            }

            if let Some(ref g) = genre {
                let g_lower = g.to_lowercase();
                let album_ids: Vec<i64> = albums.iter().map(|a| a.id).collect();
                let genre_map = queries::genres_by_album_ids(&db.conn, &album_ids)
                    .map_err(|e| super::internal_error("db", e))?;
                albums.retain(|a| {
                    genre_map
                        .get(&a.id)
                        .is_some_and(|genres| genres.iter().any(|ag| ag.contains(&g_lower)))
                });
            }

            if favourites_only {
                let fav_ids = queries::favourite_album_ids_batch(&db.conn, user)
                    .map_err(|e| super::internal_error("db", e))?;
                albums.retain(|a| fav_ids.contains(&a.id));
            }

            match sort_by {
                AlbumSortField::Title => albums.sort_by(|a, b| a.title.cmp(&b.title)),
                AlbumSortField::Date => {
                    albums.sort_by(|a, b| a.date.cmp(&b.date).then(a.title.cmp(&b.title)))
                }
                AlbumSortField::ArtistThenDate => albums.sort_by(|a, b| {
                    a.artist_name
                        .cmp(&b.artist_name)
                        .then(a.date.cmp(&b.date))
                        .then(a.title.cmp(&b.title))
                }),
                AlbumSortField::TrackCount => {
                    let album_ids: Vec<i64> = albums.iter().map(|a| a.id).collect();
                    let stats = queries::batch::album_stats(&db.conn, &album_ids)
                        .map_err(|e| super::internal_error("db", e))?;
                    let key = |id: i64| stats.get(&id).map(|s| s.track_count).unwrap_or(0);
                    albums.sort_by(|a, b| key(a.id).cmp(&key(b.id)).then(a.title.cmp(&b.title)));
                }
            }
            if sort_dir == SortDirection::Desc {
                albums.reverse();
            }

            Ok(albums)
        })
        .await?;

        paginate(
            rows.into_iter().map(|row| GqlAlbum { row }).collect(),
            after,
            first,
        )
    }

    /// Tracks matching the given filters.
    ///
    /// Every filter is a SQL predicate and the window is a `LIMIT`/`OFFSET`, so
    /// the cost of a page is the page, not the library.
    #[allow(clippy::too_many_arguments)]
    async fn tracks(
        &self,
        ctx: &Context<'_>,
        ids: Option<Vec<async_graphql::ID>>,
        album_id: Option<async_graphql::ID>,
        artist_id: Option<async_graphql::ID>,
        artist_ids: Option<Vec<async_graphql::ID>>,
        search: Option<String>,
        title: Option<String>,
        artist_name: Option<String>,
        album_title: Option<String>,
        genre: Option<String>,
        codec: Option<String>,
        source: Option<TrackSource>,
        year_start: Option<i32>,
        year_end: Option<i32>,
        min_sample_rate: Option<i32>,
        min_bit_depth: Option<i32>,
        channels: Option<i32>,
        min_duration_ms: Option<i64>,
        max_duration_ms: Option<i64>,
        #[graphql(default = false)] favourites_only: bool,
        after: Option<String>,
        first: Option<i32>,
        #[graphql(default_with = "TrackSortField::ArtistAlbumDiscTrack")] sort_by: TrackSortField,
        #[graphql(default_with = "SortDirection::Asc")] sort_dir: SortDirection,
    ) -> async_graphql::Result<Conn<GqlTrack>> {
        let ids = super::opt_row_ids(ctx, UidKind::Track, ids.as_deref()).await?;
        let album_id = super::opt_row_id(ctx, UidKind::Album, album_id.as_ref()).await?;
        let artist_id = super::opt_row_id(ctx, UidKind::Artist, artist_id.as_ref()).await?;
        let artist_ids = super::opt_row_ids(ctx, UidKind::Artist, artist_ids.as_deref()).await?;
        let artist_ids = match (artist_ids, artist_id) {
            (Some(list), _) => Some(list),
            (None, Some(one)) => Some(vec![one]),
            (None, None) => None,
        };
        let filter = TrackFilter {
            ids,
            search,
            album_id,
            artist_ids,
            title,
            artist_name,
            album_title,
            genre,
            codec,
            source: source.map(|s| s.as_db_value().to_string()),
            year_start,
            year_end,
            min_sample_rate,
            min_bit_depth,
            channels,
            min_duration_ms,
            max_duration_ms,
            favourites_of: favourites_only.then(|| super::user_id(ctx)),
        };

        let offset = page_offset(after.as_deref());
        let limit = page_size(first);
        let order = TrackOrder::from(sort_by);
        let descending = sort_dir == SortDirection::Desc;

        let rows = with_db(ctx, move |db| {
            // One row past the page tells us whether a next page exists without
            // a second COUNT(*) over the same predicate.
            queries::batch::filter_tracks(
                &db.conn,
                &filter,
                order,
                descending,
                limit as u32 + 1,
                offset as u32,
            )
            .map_err(|e| super::internal_error("db", e))
        })
        .await?;

        Ok(paginate_window(
            rows.into_iter().map(|row| GqlTrack { row }).collect(),
            offset,
            limit,
        ))
    }

    async fn track(
        &self,
        ctx: &Context<'_>,
        id: async_graphql::ID,
    ) -> async_graphql::Result<Option<GqlTrack>> {
        let id = super::row_id(ctx, UidKind::Track, &id).await?;
        with_db(ctx, move |db| {
            let row =
                queries::get_track_row(&db.conn, id).map_err(|e| super::internal_error("db", e))?;
            Ok(row.map(|row| GqlTrack { row }))
        })
        .await
    }

    async fn random_tracks(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 20)] count: i32,
        artist_id: Option<async_graphql::ID>,
        artist_ids: Option<Vec<async_graphql::ID>>,
    ) -> async_graphql::Result<Vec<GqlTrack>> {
        let artist_id = super::opt_row_id(ctx, UidKind::Artist, artist_id.as_ref()).await?;
        let artist_ids = super::opt_row_ids(ctx, UidKind::Artist, artist_ids.as_deref()).await?;
        let count = count.clamp(0, MAX_PAGE as i32) as u32;
        with_db(ctx, move |db| {
            let tracks = if let Some(ref aids) = artist_ids {
                let mut all = Vec::new();
                let per = (count / aids.len().max(1) as u32).max(1);
                for &aid in aids {
                    let mut t = queries::random_tracks(&db.conn, per, Some(aid))
                        .map_err(|e| super::internal_error("db", e))?;
                    all.append(&mut t);
                }
                all.truncate(count as usize);
                all
            } else {
                queries::random_tracks(&db.conn, count, artist_id)
                    .map_err(|e| super::internal_error("db", e))?
            };
            Ok(tracks.into_iter().map(|row| GqlTrack { row }).collect())
        })
        .await
    }

    /// What this process's own player is doing. On a server that player is
    /// headless and nobody hears it; what the user is listening to is in
    /// `clients`.
    async fn now_playing(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlNowPlaying> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        Ok(GqlNowPlaying::capture(state))
    }

    /// The play queue with derived entry statuses, download progress, and a version counter.
    async fn queue(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlQueueSnapshot> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        Ok(GqlQueueSnapshot::capture(state))
    }

    async fn library_stats(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlLibraryStats> {
        with_db(ctx, |db| {
            let stats =
                queries::library_stats(&db.conn).map_err(|e| super::internal_error("db", e))?;
            Ok(GqlLibraryStats {
                total_tracks: stats.total_tracks,
                local_tracks: stats.local_tracks,
                remote_tracks: stats.remote_tracks,
                cached_tracks: stats.cached_tracks,
                total_albums: stats.total_albums,
                total_artists: stats.total_artists,
            })
        })
        .await
    }

    /// koan apps linked to this server (a phone, a Mac), newest first. Any
    /// of them can be told what to play with `playOnClient`.
    async fn clients(&self, ctx: &Context<'_>) -> Vec<GqlClient> {
        let scope = super::client_scope(ctx);
        crate::clients::registry()
            .list(scope.as_deref())
            .into_iter()
            .map(GqlClient::from)
            .collect()
    }

    /// Albums waiting to be queued on a device when they arrive; see
    /// `queueOnClientWhenAdded`.
    async fn client_orders(&self, ctx: &Context<'_>) -> Vec<GqlClientOrder> {
        let scope = super::client_scope(ctx);
        crate::clients::registry()
            .orders(scope.as_deref())
            .into_iter()
            .map(GqlClientOrder::from)
            .collect()
    }

    async fn devices(&self) -> async_graphql::Result<Vec<GqlDevice>> {
        blocking(|| {
            let devices =
                audio::list_output_devices().map_err(|e| super::internal_error("device", e))?;
            Ok(devices
                .iter()
                .map(|d| GqlDevice {
                    name: d.name.clone(),
                    sample_rates: d.sample_rates.clone(),
                })
                .collect())
        })
        .await
    }

    async fn favourites(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        first: Option<i32>,
    ) -> async_graphql::Result<Conn<GqlTrack>> {
        let offset = page_offset(after.as_deref());
        let limit = page_size(first);
        let user = super::user_id(ctx);
        let rows = with_db(ctx, move |db| {
            let filter = TrackFilter {
                favourites_of: Some(user),
                ..Default::default()
            };
            queries::batch::filter_tracks(
                &db.conn,
                &filter,
                TrackOrder::ArtistAlbumDiscTrack,
                false,
                limit as u32 + 1,
                offset as u32,
            )
            .map_err(|e| super::internal_error("db", e))
        })
        .await?;

        Ok(paginate_window(
            rows.into_iter().map(|row| GqlTrack { row }).collect(),
            offset,
            limit,
        ))
    }

    /// The caller's share links (everyone's for an admin), newest first,
    /// expired ones included so they can be renewed or removed.
    async fn shares(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlShareLink>> {
        super::require_role(ctx, koan_core::auth::Role::User)?;
        let owner = super::share_owner(ctx);
        with_db(ctx, move |db| {
            let cfg = Config::load().unwrap_or_default();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let list = queries::shares::list_shares(&db.conn, owner)
                .map_err(|e| super::internal_error("db", e))?;
            Ok(list
                .into_iter()
                .map(|s| GqlShareLink {
                    url: cfg
                        .sharing
                        .public_url
                        .as_deref()
                        .map(|base| koan_core::helpers::share_url(base, &s.id)),
                    expired: !s.is_live(now),
                    id: s.id,
                    description: s.description,
                    created_at: s.created_at,
                    expires_at: s.expires_at,
                    visits: s.visits,
                    last_visited: s.last_visited,
                    kind: s.slice.kind.as_str().into(),
                    subject_id: s.slice.subject_id,
                    start_track_id: s.slice.start_track_id,
                    track_ids: s.track_ids,
                })
                .collect())
        })
        .await
    }

    /// The caller's playlists and everyone's public ones, in the order the
    /// owner arranged them.
    async fn playlists(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlPlaylist>> {
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            super::refresh_smart(db, user);
            let list = queries::list_playlists(&db.conn, user)
                .map_err(|e| super::internal_error("db", e))?;
            Ok(list.into_iter().map(GqlPlaylist::from).collect())
        })
        .await
    }

    /// One playlist's tracks, in playlist order. Duplicates are kept.
    async fn playlist_tracks(
        &self,
        ctx: &Context<'_>,
        id: async_graphql::ID,
    ) -> async_graphql::Result<Vec<GqlTrack>> {
        let id = super::row_id(ctx, UidKind::Playlist, &id).await?;
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            super::readable_playlist(db, user, id)?;
            match queries::smart::refresh_if_due(&db.conn, id) {
                Ok(true) => crate::clients::changed(),
                Ok(false) => {}
                Err(e) => log::warn!("smart playlist {id} not refreshed: {e}"),
            }
            let rows = queries::playlist_tracks(&db.conn, id)
                .map_err(|e| super::internal_error("db", e))?;
            Ok(rows.into_iter().map(|row| GqlTrack { row }).collect())
        })
        .await
    }

    async fn play_history(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 50)] limit: i32,
        #[graphql(default = 0)] offset: i32,
    ) -> async_graphql::Result<Vec<GqlPlayHistoryEntry>> {
        let limit = limit.clamp(0, MAX_PAGE as i32) as u32;
        let offset = offset.max(0) as u32;
        let user = super::user_id(ctx);
        with_db(ctx, move |db| {
            let entries = queries::get_play_history(&db.conn, user, limit, offset)
                .map_err(|e| super::internal_error("db", e))?;
            // One lookup for the whole page rather than one per entry.
            let ids: Vec<i64> = entries.iter().map(|e| e.track_id).collect();
            let by_id = tracks_by_id(db, ids)?;

            Ok(entries
                .into_iter()
                .map(|e| {
                    let track = by_id.get(&e.track_id).map(|t| GqlPlayHistoryTrack {
                        title: t.title.clone(),
                        artist: t.artist_name.clone(),
                        album: t.album_title.clone(),
                    });
                    GqlPlayHistoryEntry {
                        track_id: e.track_id,
                        played_at: e.played_at,
                        duration_ms: e.duration_ms,
                        track,
                    }
                })
                .collect())
        })
        .await
    }

    async fn fuzzy_search(
        &self,
        ctx: &Context<'_>,
        query: String,
        #[graphql(default_with = "FuzzySearchKind::Track")] kind: FuzzySearchKind,
        #[graphql(default = 50)] limit: i32,
    ) -> async_graphql::Result<Vec<GqlFuzzyMatch>> {
        let limit = limit.clamp(0, MAX_PAGE as i32) as usize;
        let fuzzy = ctx.data::<Arc<Fuzzy>>()?.clone();
        // Read, then hand the connection back before matching: the match
        // runs over the whole library and needs no database.
        let items = with_db(ctx, move |db| {
            let kind = match kind {
                FuzzySearchKind::Track => queries::CorpusKind::Track,
                FuzzySearchKind::Album => queries::CorpusKind::Album,
                FuzzySearchKind::Artist => queries::CorpusKind::Artist,
            };
            fuzzy
                .corpus(&db.conn, kind)
                .map_err(|e| super::internal_error("db", e))
        })
        .await?;
        super::blocking(move || {
            use nucleo::pattern::{CaseMatching, Normalization, Pattern};
            use nucleo::{Config, Matcher, Utf32Str};

            // The matcher alone, on this thread: `Nucleo` would build a
            // thread pool per request to do the same.
            let pattern = Pattern::parse(&query, CaseMatching::Ignore, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT);
            let mut buf = Vec::new();
            let mut scored: Vec<(u32, usize)> = items
                .iter()
                .enumerate()
                .filter_map(|(i, (_, text))| {
                    pattern
                        .score(Utf32Str::new(text, &mut buf), &mut matcher)
                        .map(|score| (score, i))
                })
                .collect();
            // Best first; ties to the shorter text, then corpus order, as
            // nucleo ranks them.
            scored.sort_by_key(|&(score, i)| (std::cmp::Reverse(score), items[i].1.len(), i));
            Ok(scored
                .into_iter()
                .take(limit)
                .enumerate()
                .map(|(rank, (_, i))| GqlFuzzyMatch {
                    id: items[i].0,
                    name: items[i].1.clone(),
                    rank: rank as i32,
                    kind,
                })
                .collect())
        })
        .await
    }

    async fn lyrics(
        &self,
        ctx: &Context<'_>,
        track_id: async_graphql::ID,
    ) -> async_graphql::Result<Option<GqlLyrics>> {
        use koan_core::lyrics::{self, CacheLookup};

        let track_id = super::row_id(ctx, UidKind::Track, &track_id).await?;
        let (track, lookup) = with_db(ctx, move |db| {
            let track = queries::get_track_row(&db.conn, track_id)
                .map_err(|e| super::internal_error("db", e))?
                .ok_or_else(|| {
                    async_graphql::Error::new(format!("track {} not found", track_id))
                })?;
            Ok((track, lyrics::look_up_cached(&db.conn, track_id)))
        })
        .await?;
        let found = |lyrics: lyrics::Lyrics| {
            Some(GqlLyrics {
                content: lyrics.content,
                synced: lyrics.synced,
                source: format!("{:?}", lyrics.source),
            })
        };
        let cached = match lookup {
            Ok(CacheLookup::Fresh(lyrics)) => return Ok(found(lyrics)),
            Ok(CacheLookup::Stale(cached)) => cached,
            Err(_) => return Ok(None),
        };
        // LRCLIB without a connection held: a slow answer would otherwise keep
        // one from every other resolver for as long as it took.
        let fetched = blocking(move || {
            let duration_secs = track.duration_ms.map(|d| d as u64 / 1000).unwrap_or(0);
            Ok(lyrics::fetch_from_lrclib(
                &track.artist_name,
                &track.title,
                &track.album_title,
                duration_secs,
            ))
        })
        .await?;
        with_db(ctx, move |db| {
            Ok(lyrics::settle(&db.conn, track_id, fetched, cached)
                .ok()
                .and_then(found))
        })
        .await
    }

    async fn cover_art(
        &self,
        ctx: &Context<'_>,
        track_id: async_graphql::ID,
    ) -> async_graphql::Result<Option<GqlCoverArt>> {
        let track_id = super::row_id(ctx, UidKind::Track, &track_id).await?;
        let path = with_db(ctx, move |db| {
            let track = queries::get_track_row(&db.conn, track_id)
                .map_err(|e| super::internal_error("db", e))?
                .ok_or_else(|| {
                    async_graphql::Error::new(format!("track {} not found", track_id))
                })?;
            track
                .path
                .or(track.cached_path)
                .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))
        })
        .await?;
        // Reads and parses the whole media file, so not with a connection held.
        blocking(move || {
            use base64::Engine;

            match koan_core::index::metadata::extract_cover_art(std::path::Path::new(&path)) {
                Some(data) => {
                    let mime = if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
                        "image/png"
                    } else if data.starts_with(&[0xFF, 0xD8]) {
                        "image/jpeg"
                    } else {
                        "application/octet-stream"
                    };
                    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                    Ok(Some(GqlCoverArt {
                        data_base64: encoded,
                        mime: mime.into(),
                    }))
                }
                None => Ok(None),
            }
        })
        .await
    }

    /// Current visualizer frame — spectrum, peaks, VU levels, beat energy, waveform.
    /// Returns None if no VizSnapshot is available (headless without analyzer).
    async fn viz_frame(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            default = false,
            desc = "Include raw waveform samples (4096 interleaved stereo floats)."
        )]
        include_waveform: bool,
    ) -> async_graphql::Result<Option<GqlVizFrame>> {
        let viz = match ctx.data_opt::<Arc<VizSnapshot>>() {
            Some(v) => v,
            None => return Ok(None),
        };
        let frame = viz.read();
        Ok(Some(GqlVizFrame {
            spectrum: frame.spectrum.to_vec(),
            peaks: frame.peaks.to_vec(),
            vu_levels: frame.vu_levels.to_vec(),
            beat_energy: frame.beat_energy,
            waveform: if include_waveform {
                frame.waveform.clone()
            } else {
                Vec::new()
            },
        }))
    }

    /// A background job started by `triggerScan` or `triggerRemoteSync`.
    async fn job(&self, ctx: &Context<'_>, id: String) -> async_graphql::Result<Option<GqlJob>> {
        let registry = ctx.data::<JobRegistry>()?;
        Ok(registry.get(&id).map(GqlJob::from))
    }

    /// Background jobs started by this process, oldest first.
    async fn jobs(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlJob>> {
        let registry = ctx.data::<JobRegistry>()?;
        Ok(registry.list().into_iter().map(GqlJob::from).collect())
    }

    /// Current configuration.
    async fn config(&self) -> async_graphql::Result<GqlConfig> {
        blocking(move || {
            let cfg = Config::load().unwrap_or_default();
            Ok(GqlConfig {
                library_folders: cfg
                    .library
                    .folders
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
                replaygain_mode: format!("{:?}", cfg.playback.replaygain).to_lowercase(),
                pre_amp_db: cfg.playback.pre_amp_db,
                output_device: cfg.playback.output_device.clone(),
                target_fps: cfg.playback.target_fps as i32,
                art_size: cfg.playback.art_size as i32,
                remote_enabled: cfg.remote.enabled,
                remote_url: cfg.remote.url.clone(),
                remote_username: cfg.remote.username.clone(),
                cache_limit: cfg.remote.cache_limit.clone(),
                visualizer_fps: cfg.visualizer.fps as i32,
                graphql_port: cfg.graphql.port as i32,
                graphql_playground: cfg.graphql.playground,
            })
        })
        .await
    }

    /// Every account on this server. Admins only.
    async fn users(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlUser>> {
        super::require_role(ctx, koan_core::auth::Role::Admin)?;
        with_db(ctx, |db| {
            Ok(queries::auth::list_users(&db.conn)?
                .into_iter()
                .map(|u| GqlUser {
                    username: u.username,
                    role: u.role.into(),
                    created_at: u.created_at,
                })
                .collect())
        })
        .await
    }

    /// Playlist version counter — bumped on every mutation. Use for change detection.
    async fn playlist_version(&self, ctx: &Context<'_>) -> async_graphql::Result<u64> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        Ok(state.playlist_version())
    }
}

/// Fetch a set of tracks by ID in one statement.
fn tracks_by_id(
    db: &koan_core::db::connection::Database,
    ids: Vec<i64>,
) -> async_graphql::Result<std::collections::HashMap<i64, queries::TrackRow>> {
    let count = ids.len().max(1) as u32;
    let filter = TrackFilter {
        ids: Some(ids),
        ..Default::default()
    };
    let rows = queries::batch::filter_tracks(&db.conn, &filter, TrackOrder::Title, false, count, 0)
        .map_err(|e| super::internal_error("db", e))?;
    Ok(rows.into_iter().map(|t| (t.id, t)).collect())
}

impl From<TrackSortField> for TrackOrder {
    fn from(field: TrackSortField) -> Self {
        match field {
            TrackSortField::Title => TrackOrder::Title,
            TrackSortField::Artist => TrackOrder::Artist,
            TrackSortField::Album => TrackOrder::Album,
            TrackSortField::Duration => TrackOrder::Duration,
            TrackSortField::ArtistAlbumDiscTrack => TrackOrder::ArtistAlbumDiscTrack,
        }
    }
}

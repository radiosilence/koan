use std::sync::Arc;

use async_graphql::{Context, Object};
use koan_core::audio;
use koan_core::audio::viz::VizSnapshot;
use koan_core::config::Config;
use koan_core::db::queries;
use koan_core::db::queries::batch::{TrackFilter, TrackOrder};
use koan_core::player::state::SharedPlayerState;

use super::helpers::{MAX_PAGE, album_year, page_offset, page_size, paginate, paginate_window};
use super::jobs::JobRegistry;
use super::types::*;
use super::{blocking, with_db};

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
        ids: Option<Vec<i64>>,
        search: Option<String>,
        genre: Option<String>,
        #[graphql(default = false)] favourites_only: bool,
        after: Option<String>,
        first: Option<i32>,
        #[graphql(default_with = "ArtistSortField::Name")] sort_by: ArtistSortField,
        #[graphql(default_with = "SortDirection::Asc")] sort_dir: SortDirection,
    ) -> async_graphql::Result<Conn<GqlArtist>> {
        let rows = with_db(ctx, move |db| {
            let mut artists = if let Some(ref query) = search {
                queries::find_artists(&db.conn, query)
                    .map_err(|e| super::internal_error("db", e))?
            } else {
                queries::all_artists(&db.conn).map_err(|e| super::internal_error("db", e))?
            };

            if let Some(ref id_list) = ids {
                artists.retain(|a| id_list.contains(&a.id));
            }

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
                let fav_ids = queries::favourite_artist_ids_batch(&db.conn)
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
        ids: Option<Vec<i64>>,
        artist_id: Option<i64>,
        artist_ids: Option<Vec<i64>>,
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
        let rows = with_db(ctx, move |db| {
            let mut albums = if let Some(aid) = artist_id {
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
                let fav_ids = queries::favourite_album_ids_batch(&db.conn)
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
        ids: Option<Vec<i64>>,
        album_id: Option<i64>,
        artist_id: Option<i64>,
        artist_ids: Option<Vec<i64>>,
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
            favourites_only,
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

    async fn track(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GqlTrack>> {
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
        artist_id: Option<i64>,
        artist_ids: Option<Vec<i64>>,
    ) -> async_graphql::Result<Vec<GqlTrack>> {
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
        let rows = with_db(ctx, move |db| {
            let filter = TrackFilter {
                favourites_only: true,
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

    /// Every playlist, in the order the owner arranged them.
    async fn playlists(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlPlaylist>> {
        with_db(ctx, |db| {
            let list =
                queries::list_playlists(&db.conn).map_err(|e| super::internal_error("db", e))?;
            Ok(list.into_iter().map(GqlPlaylist::from).collect())
        })
        .await
    }

    /// One playlist's tracks, in playlist order. Duplicates are kept.
    async fn playlist_tracks(
        &self,
        ctx: &Context<'_>,
        id: i64,
    ) -> async_graphql::Result<Vec<GqlTrack>> {
        with_db(ctx, move |db| {
            let rows = queries::playlist_tracks(&db.conn, id)
                .map_err(|e| super::internal_error("db", e))?;
            Ok(rows.into_iter().map(|row| GqlTrack { row }).collect())
        })
        .await
    }

    async fn radio_status(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlRadioStatus> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        Ok(GqlRadioStatus {
            enabled: state.radio_mode(),
        })
    }

    async fn similar_artists(
        &self,
        ctx: &Context<'_>,
        artist_id: i64,
    ) -> async_graphql::Result<Vec<GqlSimilarArtist>> {
        with_db(ctx, move |db| {
            let entries = queries::get_similar_artists_detailed(&db.conn, artist_id)
                .map_err(|e| super::internal_error("db", e))?;
            Ok(entries
                .into_iter()
                .map(|e| GqlSimilarArtist {
                    artist: GqlSimilarArtistInfo {
                        id: e.artist.id,
                        name: e.artist.name,
                    },
                    score: e.score,
                    source: e.source,
                    relationship: e.relationship,
                })
                .collect())
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
        with_db(ctx, move |db| {
            let entries = queries::get_play_history(&db.conn, limit, offset)
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
        with_db(ctx, move |db| {
            use nucleo::pattern::{CaseMatching, Normalization};
            use nucleo::{Config, Nucleo};

            // Build (id, match_text) pairs based on kind.
            let items: Vec<(i64, String)> = match kind {
                FuzzySearchKind::Track => {
                    let tracks = queries::all_tracks(&db.conn)
                        .map_err(|e| super::internal_error("db", e))?;
                    tracks
                        .into_iter()
                        .map(|t| {
                            (
                                t.id,
                                format!("{} — {} — {}", t.artist_name, t.album_title, t.title),
                            )
                        })
                        .collect()
                }
                FuzzySearchKind::Album => {
                    let albums = queries::all_albums(&db.conn)
                        .map_err(|e| super::internal_error("db", e))?;
                    albums
                        .into_iter()
                        .map(|a| (a.id, format!("{} — {}", a.artist_name, a.title)))
                        .collect()
                }
                FuzzySearchKind::Artist => {
                    let artists = queries::all_artists(&db.conn)
                        .map_err(|e| super::internal_error("db", e))?;
                    artists.into_iter().map(|a| (a.id, a.name)).collect()
                }
            };

            // Run nucleo fuzzy matching.
            let mut nucleo: Nucleo<u32> =
                Nucleo::new(Config::DEFAULT, std::sync::Arc::new(|| {}), None, 1);
            let injector = nucleo.injector();
            for (i, (_id, text)) in items.iter().enumerate() {
                let text = text.clone();
                injector.push(i as u32, |_val, cols| {
                    cols[0] = text.into();
                });
            }

            // Parse pattern and tick until matching settles.
            nucleo
                .pattern
                .reparse(0, &query, CaseMatching::Smart, Normalization::Smart, false);
            // Tick enough times for matching to complete on the dataset.
            for _ in 0..20 {
                nucleo.tick(10);
            }

            let snap = nucleo.snapshot();
            let count = (snap.matched_item_count() as usize).min(limit);
            let mut results = Vec::with_capacity(count);
            for i in 0..count as u32 {
                if let Some(item) = snap.get_matched_item(i) {
                    let idx = *item.data as usize;
                    if idx < items.len() {
                        results.push(GqlFuzzyMatch {
                            id: items[idx].0,
                            name: items[idx].1.clone(),
                            rank: i as i32,
                            kind,
                        });
                    }
                }
            }
            Ok(results)
        })
        .await
    }

    async fn lyrics(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<Option<GqlLyrics>> {
        with_db(ctx, move |db| {
            let track = queries::get_track_row(&db.conn, track_id)
                .map_err(|e| super::internal_error("db", e))?
                .ok_or_else(|| {
                    async_graphql::Error::new(format!("track {} not found", track_id))
                })?;
            let duration_secs = track.duration_ms.map(|d| d as u64 / 1000).unwrap_or(0);
            // Falls through to a blocking LRCLIB fetch when nothing is cached.
            match koan_core::lyrics::fetch_lyrics(
                &db.conn,
                track_id,
                &track.artist_name,
                &track.title,
                &track.album_title,
                duration_secs,
            ) {
                Ok(lyrics) => Ok(Some(GqlLyrics {
                    content: lyrics.content,
                    synced: lyrics.synced,
                    source: format!("{:?}", lyrics.source),
                })),
                Err(_) => Ok(None),
            }
        })
        .await
    }

    async fn similar_tracks(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
        #[graphql(default = 20)] limit: i32,
    ) -> async_graphql::Result<Vec<GqlSimilarTrack>> {
        let limit = limit.clamp(0, MAX_PAGE as i32) as usize;
        with_db(ctx, move |db| {
            let results = queries::find_similar(&db.conn, track_id, limit)
                .map_err(|e| super::internal_error("db", e))?;
            let by_id = tracks_by_id(db, results.iter().map(|(tid, _)| *tid).collect())?;

            Ok(results
                .into_iter()
                .filter_map(|(tid, dist)| {
                    by_id.get(&tid).map(|row| GqlSimilarTrack {
                        row: row.clone(),
                        distance: dist as f64,
                    })
                })
                .collect())
        })
        .await
    }

    async fn cover_art(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<Option<GqlCoverArt>> {
        with_db(ctx, move |db| {
            use base64::Engine;

            let track = queries::get_track_row(&db.conn, track_id)
                .map_err(|e| super::internal_error("db", e))?
                .ok_or_else(|| {
                    async_graphql::Error::new(format!("track {} not found", track_id))
                })?;
            let path = track
                .path
                .as_ref()
                .or(track.cached_path.as_ref())
                .ok_or_else(|| {
                    async_graphql::Error::new(format!("track {} has no path", track_id))
                })?;

            // Reads and parses the whole media file.
            match koan_core::index::metadata::extract_cover_art(std::path::Path::new(path)) {
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
    async fn config(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlConfig> {
        let radio_enabled = ctx.data::<Arc<SharedPlayerState>>()?.radio_mode();
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
                transcode_quality: cfg.remote.transcode_quality.clone(),
                cache_limit: cfg.remote.cache_limit.clone(),
                visualizer_fps: cfg.visualizer.fps as i32,
                radio_enabled,
                graphql_port: cfg.graphql.port as i32,
                graphql_playground: cfg.graphql.playground,
            })
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

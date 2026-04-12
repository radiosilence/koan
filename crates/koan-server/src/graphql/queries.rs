use std::sync::Arc;

use async_graphql::connection::{Connection, EmptyFields};
use async_graphql::{Context, Object};
use koan_core::audio;
use koan_core::audio::viz::VizSnapshot;
use koan_core::config::Config;
use koan_core::db::queries;
use koan_core::player::state::{PlaybackState, SharedPlayerState};

use super::DbHandle;
use super::helpers::{album_year, paginate, track_year};
use super::types::*;

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
        #[graphql(default_with = "ArtistSortField::Name")] _sort_by: ArtistSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<Connection<usize, GqlArtist, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let mut artists = if let Some(ref query) = search {
            queries::find_artists(&db.conn, query)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else {
            queries::all_artists(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        };

        if let Some(ref id_list) = ids {
            artists.retain(|a| id_list.contains(&a.id));
        }

        if let Some(ref g) = genre {
            let g_lower = g.to_lowercase();
            let artist_ids: Vec<i64> = artists.iter().map(|a| a.id).collect();
            let genre_map = queries::genres_by_artist_ids(&db.conn, &artist_ids)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
            artists.retain(|a| {
                genre_map
                    .get(&a.id)
                    .is_some_and(|genres| genres.iter().any(|ag| ag.contains(&g_lower)))
            });
        }

        if favourites_only {
            let fav_ids = queries::favourite_artist_ids_batch(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
            artists.retain(|a| fav_ids.contains(&a.id));
        }

        paginate(
            artists.into_iter().map(|row| GqlArtist { row }).collect(),
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
        #[graphql(default_with = "AlbumSortField::ArtistThenDate")] _sort_by: AlbumSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<Connection<usize, GqlAlbum, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;

        let mut albums = if let Some(aid) = artist_id {
            queries::albums_for_artist(&db.conn, aid)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else if let Some(ref aids) = artist_ids {
            let mut all = Vec::new();
            for &aid in aids {
                let mut a = queries::albums_for_artist(&db.conn, aid)
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
                all.append(&mut a);
            }
            all
        } else {
            queries::all_albums(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
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
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
            albums.retain(|a| {
                genre_map
                    .get(&a.id)
                    .is_some_and(|genres| genres.iter().any(|ag| ag.contains(&g_lower)))
            });
        }

        if favourites_only {
            let fav_ids = queries::favourite_album_ids_batch(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
            albums.retain(|a| fav_ids.contains(&a.id));
        }

        paginate(
            albums.into_iter().map(|row| GqlAlbum { row }).collect(),
            after,
            first,
        )
    }

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
        #[graphql(default_with = "TrackSortField::ArtistAlbumDiscTrack")] _sort_by: TrackSortField,
        #[graphql(default_with = "SortDirection::Asc")] _sort_dir: SortDirection,
    ) -> async_graphql::Result<Connection<usize, GqlTrack, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;

        let mut tracks = if let Some(ref query) = search {
            queries::search_tracks_paged(&db.conn, query, 10000, 0)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else if let Some(album) = album_id {
            queries::tracks_for_album(&db.conn, album)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else if let Some(ref aids) = artist_ids {
            let mut all = Vec::new();
            for &aid in aids {
                let mut t = queries::tracks_for_artist(&db.conn, aid)
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
                all.append(&mut t);
            }
            all
        } else if let Some(aid) = artist_id {
            queries::tracks_for_artist(&db.conn, aid)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        } else {
            queries::all_tracks(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
        };

        if let Some(ref id_list) = ids {
            tracks.retain(|t| id_list.contains(&t.id));
        }

        if let Some(ref t) = title {
            let t_lower = t.to_lowercase();
            tracks.retain(|tr| tr.title.to_lowercase().contains(&t_lower));
        }

        if let Some(ref a) = artist_name {
            let a_lower = a.to_lowercase();
            tracks.retain(|tr| {
                tr.artist_name.to_lowercase().contains(&a_lower)
                    || tr.album_artist_name.to_lowercase().contains(&a_lower)
            });
        }

        if let Some(ref al) = album_title {
            let al_lower = al.to_lowercase();
            tracks.retain(|tr| tr.album_title.to_lowercase().contains(&al_lower));
        }

        if let Some(ref g) = genre {
            let g_lower = g.to_lowercase();
            tracks.retain(|tr| {
                tr.genre
                    .as_ref()
                    .map(|tg| tg.to_lowercase().contains(&g_lower))
                    .unwrap_or(false)
            });
        }

        if let Some(ref c) = codec {
            let c_lower = c.to_lowercase();
            tracks.retain(|tr| {
                tr.codec
                    .as_ref()
                    .map(|tc| tc.to_lowercase().contains(&c_lower))
                    .unwrap_or(false)
            });
        }

        if let Some(src) = source {
            let src_str = match src {
                TrackSource::Local => "local",
                TrackSource::Remote => "remote",
                TrackSource::Cached => "cached",
            };
            tracks.retain(|t| t.source == src_str);
        }

        if year_start.is_some() || year_end.is_some() {
            tracks.retain(|t| {
                let y = track_year(&db, t);
                match y {
                    Some(year) => {
                        year_start.is_none_or(|ys| year >= ys)
                            && year_end.is_none_or(|ye| year <= ye)
                    }
                    None => false,
                }
            });
        }

        if let Some(sr) = min_sample_rate {
            tracks.retain(|t| t.sample_rate.map(|v| v >= sr).unwrap_or(false));
        }

        if let Some(bd) = min_bit_depth {
            tracks.retain(|t| t.bit_depth.map(|v| v >= bd).unwrap_or(false));
        }

        if let Some(ch) = channels {
            tracks.retain(|t| t.channels == Some(ch));
        }

        if let Some(min_d) = min_duration_ms {
            tracks.retain(|t| t.duration_ms.map(|d| d >= min_d).unwrap_or(false));
        }

        if let Some(max_d) = max_duration_ms {
            tracks.retain(|t| t.duration_ms.map(|d| d <= max_d).unwrap_or(false));
        }

        if favourites_only {
            let fav_paths = queries::load_favourites(&db.conn)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
            tracks.retain(|t| {
                t.path
                    .as_ref()
                    .or(t.cached_path.as_ref())
                    .map(|p| fav_paths.contains(std::path::Path::new(p)))
                    .unwrap_or(false)
            });
        }

        paginate(
            tracks.into_iter().map(|row| GqlTrack { row }).collect(),
            after,
            first,
        )
    }

    async fn track(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<GqlTrack>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let row = queries::get_track_row(&db.conn, id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(row.map(|row| GqlTrack { row }))
    }

    async fn random_tracks(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 20)] count: i32,
        artist_id: Option<i64>,
        artist_ids: Option<Vec<i64>>,
    ) -> async_graphql::Result<Vec<GqlTrack>> {
        let db = ctx.data::<DbHandle>()?.open()?;

        if let Some(ref aids) = artist_ids {
            let mut all = Vec::new();
            let per = (count as u32 / aids.len() as u32).max(1);
            for &aid in aids {
                let mut t = queries::random_tracks(&db.conn, per, Some(aid))
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
                all.append(&mut t);
            }
            all.truncate(count as usize);
            Ok(all.into_iter().map(|row| GqlTrack { row }).collect())
        } else {
            let tracks = queries::random_tracks(&db.conn, count as u32, artist_id)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
            Ok(tracks.into_iter().map(|row| GqlTrack { row }).collect())
        }
    }

    async fn now_playing(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlNowPlaying> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let playback_state = match state.playback_state() {
            PlaybackState::Stopped => PlaybackStateEnum::Stopped,
            PlaybackState::Playing => PlaybackStateEnum::Playing,
            PlaybackState::Paused => PlaybackStateEnum::Paused,
        };
        let position_ms = state.position_ms();
        let (track, queue_item_id, duration_ms) = if let Some(info) = state.track_info() {
            let (items, _cursor) = state.snapshot_playlist();
            let playlist_item = items.iter().find(|i| i.id == info.id);
            let track = GqlNowPlayingTrack {
                title: playlist_item.map(|i| i.title.clone()).unwrap_or_default(),
                artist: playlist_item.map(|i| i.artist.clone()).unwrap_or_default(),
                album: playlist_item.map(|i| i.album.clone()).unwrap_or_default(),
                codec: info.codec.clone(),
                sample_rate: info.sample_rate,
                bit_depth: info.bit_depth,
                bitrate_kbps: info.bitrate_kbps,
                channels: info.channels,
                duration_ms: info.duration_ms,
            };
            (
                Some(track),
                Some(info.id.0.to_string()),
                Some(info.duration_ms),
            )
        } else {
            (None, None, None)
        };

        Ok(GqlNowPlaying {
            state: playback_state,
            position_ms,
            duration_ms,
            track,
            queue_item_id,
        })
    }

    /// The play queue with derived entry statuses, download progress, and a version counter.
    async fn queue(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlQueueSnapshot> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let version = state.playlist_version();
        let snap = state.derive_visible_queue();

        let entries = snap
            .entries
            .iter()
            .map(|entry| {
                use koan_core::player::state::QueueEntryStatus;

                let status = match entry.status {
                    QueueEntryStatus::Queued => GqlQueueEntryStatus::Queued,
                    QueueEntryStatus::Playing => GqlQueueEntryStatus::Playing,
                    QueueEntryStatus::Played => GqlQueueEntryStatus::Played,
                    QueueEntryStatus::Downloading => GqlQueueEntryStatus::Downloading,
                    QueueEntryStatus::PriorityPending => GqlQueueEntryStatus::PriorityPending,
                    QueueEntryStatus::Failed => GqlQueueEntryStatus::Failed,
                };

                let download_progress = entry
                    .download_progress
                    .map(|(downloaded, total)| GqlDownloadProgress { downloaded, total });

                GqlQueueEntry {
                    queue_item_id: entry.id.0.to_string(),
                    title: entry.title.clone(),
                    artist: entry.artist.clone(),
                    album: entry.album.clone(),
                    codec: entry.codec.clone(),
                    track_number: entry.track_number,
                    disc: entry.disc,
                    duration_ms: entry.duration_ms,
                    is_current: entry.status == QueueEntryStatus::Playing,
                    status,
                    download_progress,
                }
            })
            .collect();

        Ok(GqlQueueSnapshot {
            version,
            entries,
            finished_count: snap.finished_count as i32,
            has_playing: snap.has_playing,
            queue_count: snap.queue_count as i32,
        })
    }

    async fn library_stats(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlLibraryStats> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let stats = queries::library_stats(&db.conn)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(GqlLibraryStats {
            total_tracks: stats.total_tracks,
            local_tracks: stats.local_tracks,
            remote_tracks: stats.remote_tracks,
            cached_tracks: stats.cached_tracks,
            total_albums: stats.total_albums,
            total_artists: stats.total_artists,
        })
    }

    async fn devices(&self) -> async_graphql::Result<Vec<GqlDevice>> {
        let devices = audio::list_output_devices()
            .map_err(|e| async_graphql::Error::new(format!("device error: {}", e)))?;
        Ok(devices
            .iter()
            .map(|d| GqlDevice {
                name: d.name.clone(),
                sample_rates: d.sample_rates.clone(),
            })
            .collect())
    }

    async fn favourites(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        first: Option<i32>,
    ) -> async_graphql::Result<Connection<usize, GqlTrack, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let fav_paths = queries::load_favourites(&db.conn)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        let mut tracks = Vec::new();
        for path in &fav_paths {
            let path_str = path.to_string_lossy();
            if let Ok(Some(tid)) = queries::track_id_by_path(&db.conn, &path_str)
                && let Ok(Some(row)) = queries::get_track_row(&db.conn, tid)
            {
                tracks.push(GqlTrack { row });
            }
        }
        paginate(tracks, after, first)
    }

    async fn snapshots(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlSnapshot>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let list = queries::list_snapshots(&db.conn)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(list
            .into_iter()
            .map(|s| GqlSnapshot {
                name: s.name,
                track_count: s.track_count as i32,
                position_ms: s.position_ms,
                created_at: s.created_at,
            })
            .collect())
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
        let db = ctx.data::<DbHandle>()?.open()?;
        let entries = queries::get_similar_artists_detailed(&db.conn, artist_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
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
    }

    async fn play_history(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 50)] limit: i32,
        #[graphql(default = 0)] offset: i32,
    ) -> async_graphql::Result<Vec<GqlPlayHistoryEntry>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let entries = queries::get_play_history(&db.conn, limit as u32, offset as u32)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(entries
            .into_iter()
            .map(|e| {
                let track = queries::get_track_row(&db.conn, e.track_id)
                    .ok()
                    .flatten()
                    .map(|t| GqlPlayHistoryTrack {
                        title: t.title,
                        artist: t.artist_name,
                        album: t.album_title,
                    });
                GqlPlayHistoryEntry {
                    track_id: e.track_id,
                    played_at: e.played_at,
                    duration_ms: e.duration_ms,
                    track,
                }
            })
            .collect())
    }

    async fn fuzzy_search(
        &self,
        ctx: &Context<'_>,
        query: String,
        #[graphql(default_with = "FuzzySearchKind::Track")] kind: FuzzySearchKind,
        #[graphql(default = 50)] limit: i32,
    ) -> async_graphql::Result<Vec<GqlFuzzyMatch>> {
        use nucleo::pattern::{CaseMatching, Normalization};
        use nucleo::{Config, Nucleo};

        let db = ctx.data::<DbHandle>()?.open()?;

        // Build (id, match_text) pairs based on kind.
        let items: Vec<(i64, String)> = match kind {
            FuzzySearchKind::Track => {
                let tracks = queries::all_tracks(&db.conn)
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
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
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
                albums
                    .into_iter()
                    .map(|a| (a.id, format!("{} — {}", a.artist_name, a.title)))
                    .collect()
            }
            FuzzySearchKind::Artist => {
                let artists = queries::all_artists(&db.conn)
                    .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
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
        let count = (snap.matched_item_count() as usize).min(limit as usize);
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
    }

    async fn lyrics(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<Option<GqlLyrics>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let duration_secs = track.duration_ms.map(|d| d as u64 / 1000).unwrap_or(0);
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
    }

    async fn similar_tracks(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
        #[graphql(default = 20)] limit: i32,
    ) -> async_graphql::Result<Vec<GqlSimilarTrack>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let results = queries::find_similar(&db.conn, track_id, limit as usize)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        let mut out = Vec::with_capacity(results.len());
        for (tid, dist) in results {
            if let Ok(Some(row)) = queries::get_track_row(&db.conn, tid) {
                out.push(GqlSimilarTrack {
                    row,
                    distance: dist as f64,
                });
            }
        }
        Ok(out)
    }

    async fn cover_art(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<Option<GqlCoverArt>> {
        use base64::Engine;

        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;

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

    /// Current configuration.
    async fn config(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlConfig> {
        let cfg = Config::load().unwrap_or_default();
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
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
            radio_enabled: state.radio_mode(),
            graphql_port: cfg.graphql.port as i32,
            graphql_playground: cfg.graphql.playground,
        })
    }

    /// Playlist version counter — bumped on every mutation. Use for change detection.
    async fn playlist_version(&self, ctx: &Context<'_>) -> async_graphql::Result<u64> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        Ok(state.playlist_version())
    }
}

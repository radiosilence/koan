use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_graphql::connection::{Connection, Edge, EmptyFields};
use async_graphql::{Context, EmptySubscription, Enum, Object, Schema, SimpleObject};
use crossbeam_channel::Sender;
use koan_core::audio::device;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::queries;
use koan_core::db::queries::playback_state::PersistedQueueItem;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, QueueItemId, SharedPlayerState};
use uuid::Uuid;

use super::open_db;

// ---------------------------------------------------------------------------
// Pagination helper — uses usize as cursor (async-graphql has built-in impl)
// ---------------------------------------------------------------------------

fn paginate<T: async_graphql::OutputType>(
    items: Vec<T>,
    after: Option<String>,
    first: Option<i32>,
) -> async_graphql::Result<Connection<usize, T, EmptyFields, EmptyFields>> {
    let total = items.len();

    let start = if let Some(ref cursor) = after {
        cursor.parse::<usize>().unwrap_or(0) + 1
    } else {
        0
    };

    let end = if let Some(f) = first {
        (start + f as usize).min(total)
    } else {
        total
    };

    let mut conn = Connection::new(start > 0, end < total);
    for (i, item) in items.into_iter().enumerate().skip(start).take(end - start) {
        conn.edges.push(Edge::new(i, item));
    }
    Ok(conn)
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum PlaybackStateEnum {
    Stopped,
    Playing,
    Paused,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum TrackSource {
    Local,
    Remote,
    Cached,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum ArtistSortField {
    Name,
    TrackCount,
    AlbumCount,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum AlbumSortField {
    Title,
    Date,
    ArtistThenDate,
    TrackCount,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum TrackSortField {
    Title,
    Artist,
    Album,
    Duration,
    ArtistAlbumDiscTrack,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
enum SortDirection {
    Asc,
    Desc,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

struct GqlArtist {
    row: queries::ArtistRow,
}

#[Object]
impl GqlArtist {
    async fn id(&self) -> i64 {
        self.row.id
    }

    async fn name(&self) -> &str {
        &self.row.name
    }

    async fn albums(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        first: Option<i32>,
    ) -> async_graphql::Result<Connection<usize, GqlAlbum, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let all = queries::albums_for_artist(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        paginate(
            all.into_iter().map(|row| GqlAlbum { row }).collect(),
            after,
            first,
        )
    }

    async fn tracks(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        first: Option<i32>,
    ) -> async_graphql::Result<Connection<usize, GqlTrack, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let all = queries::tracks_for_artist(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        paginate(
            all.into_iter().map(|row| GqlTrack { row }).collect(),
            after,
            first,
        )
    }

    async fn album_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let albums = queries::albums_for_artist(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(albums.len() as i32)
    }

    async fn track_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tracks = queries::tracks_for_artist(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(tracks.len() as i32)
    }
}

struct GqlAlbum {
    row: queries::AlbumRow,
}

#[Object]
impl GqlAlbum {
    async fn id(&self) -> i64 {
        self.row.id
    }

    async fn title(&self) -> &str {
        &self.row.title
    }

    async fn artist_id(&self) -> i64 {
        self.row.artist_id
    }

    async fn artist_name(&self) -> &str {
        &self.row.artist_name
    }

    async fn date(&self) -> Option<&str> {
        self.row.date.as_deref()
    }

    async fn codec(&self) -> Option<&str> {
        self.row.codec.as_deref()
    }

    async fn label(&self) -> Option<&str> {
        self.row.label.as_deref()
    }

    async fn disc_count(&self) -> Option<i32> {
        self.row.total_discs
    }

    async fn tracks(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        first: Option<i32>,
    ) -> async_graphql::Result<Connection<usize, GqlTrack, EmptyFields, EmptyFields>> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let all = queries::tracks_for_album(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        paginate(
            all.into_iter().map(|row| GqlTrack { row }).collect(),
            after,
            first,
        )
    }

    async fn track_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tracks = queries::tracks_for_album(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(tracks.len() as i32)
    }

    async fn total_duration_ms(&self, ctx: &Context<'_>) -> async_graphql::Result<i64> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tracks = queries::tracks_for_album(&db.conn, self.row.id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        Ok(tracks.iter().filter_map(|t| t.duration_ms).sum())
    }
}

struct GqlTrack {
    row: queries::TrackRow,
}

#[Object]
impl GqlTrack {
    async fn id(&self) -> i64 {
        self.row.id
    }

    async fn title(&self) -> &str {
        &self.row.title
    }

    async fn artist(&self) -> &str {
        &self.row.artist_name
    }

    async fn album_artist(&self) -> &str {
        &self.row.album_artist_name
    }

    async fn album(&self) -> &str {
        &self.row.album_title
    }

    async fn album_id(&self) -> Option<i64> {
        self.row.album_id
    }

    async fn artist_id(&self) -> Option<i64> {
        self.row.artist_id
    }

    async fn disc(&self) -> Option<i32> {
        self.row.disc
    }

    async fn track_number(&self) -> Option<i32> {
        self.row.track_number
    }

    async fn duration_ms(&self) -> Option<i64> {
        self.row.duration_ms
    }

    async fn codec(&self) -> Option<&str> {
        self.row.codec.as_deref()
    }

    async fn sample_rate(&self) -> Option<i32> {
        self.row.sample_rate
    }

    async fn bit_depth(&self) -> Option<i32> {
        self.row.bit_depth
    }

    async fn channels(&self) -> Option<i32> {
        self.row.channels
    }

    async fn bitrate(&self) -> Option<i32> {
        self.row.bitrate
    }

    async fn genre(&self) -> Option<&str> {
        self.row.genre.as_deref()
    }

    async fn source(&self) -> TrackSource {
        match self.row.source.as_str() {
            "local" => TrackSource::Local,
            "cached" => TrackSource::Cached,
            _ => TrackSource::Remote,
        }
    }

    async fn remote_id(&self) -> Option<&str> {
        self.row.remote_id.as_deref()
    }

    async fn path(&self) -> Option<&str> {
        self.row.path.as_deref()
    }

    async fn cached_path(&self) -> Option<&str> {
        self.row.cached_path.as_deref()
    }

    async fn is_favourite(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let favs = queries::load_favourites(&db.conn)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let path = self
            .row
            .path
            .as_ref()
            .or(self.row.cached_path.as_ref())
            .map(std::path::PathBuf::from);
        Ok(path.map(|p| favs.contains(&p)).unwrap_or(false))
    }
}

#[derive(SimpleObject)]
struct GqlNowPlaying {
    state: PlaybackStateEnum,
    position_ms: u64,
    duration_ms: Option<u64>,
    track: Option<GqlNowPlayingTrack>,
    queue_item_id: Option<String>,
}

#[derive(SimpleObject)]
struct GqlNowPlayingTrack {
    title: String,
    artist: String,
    album: String,
    codec: String,
    sample_rate: u32,
    bit_depth: u16,
    channels: u16,
    duration_ms: u64,
}

struct GqlQueueEntry {
    queue_item_id: String,
    title: String,
    artist: String,
    album: String,
    codec: Option<String>,
    track_number: Option<i64>,
    disc: Option<i64>,
    duration_ms: Option<u64>,
    is_current: bool,
}

#[Object]
impl GqlQueueEntry {
    async fn queue_item_id(&self) -> &str {
        &self.queue_item_id
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn artist(&self) -> &str {
        &self.artist
    }

    async fn album(&self) -> &str {
        &self.album
    }

    async fn codec(&self) -> Option<&str> {
        self.codec.as_deref()
    }

    async fn track_number(&self) -> Option<i64> {
        self.track_number
    }

    async fn disc(&self) -> Option<i64> {
        self.disc
    }

    async fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    async fn is_current(&self) -> bool {
        self.is_current
    }
}

#[derive(SimpleObject)]
struct GqlLibraryStats {
    total_tracks: i64,
    local_tracks: i64,
    remote_tracks: i64,
    cached_tracks: i64,
    total_albums: i64,
    total_artists: i64,
}

#[derive(SimpleObject)]
struct GqlDevice {
    name: String,
    sample_rates: Vec<f64>,
}

#[derive(SimpleObject)]
struct GqlSimilarArtist {
    artist: GqlSimilarArtistInfo,
    score: f64,
    source: String,
    relationship: String,
}

#[derive(SimpleObject)]
struct GqlSimilarArtistInfo {
    id: i64,
    name: String,
}

#[derive(SimpleObject)]
struct GqlPlayHistoryEntry {
    track_id: i64,
    played_at: i64,
    duration_ms: Option<i64>,
    track: Option<GqlPlayHistoryTrack>,
}

#[derive(SimpleObject)]
struct GqlPlayHistoryTrack {
    title: String,
    artist: String,
    album: String,
}

#[derive(SimpleObject)]
struct GqlSnapshot {
    name: String,
    track_count: i32,
    position_ms: u64,
    created_at: String,
}

#[derive(SimpleObject)]
struct GqlRadioStatus {
    enabled: bool,
}

#[derive(SimpleObject)]
struct GqlShare {
    id: String,
    url: Option<String>,
    description: Option<String>,
    expires: Option<String>,
    visit_count: i64,
}

/// Mutation/query result status.
struct GqlStatus {
    success: bool,
    message: String,
}

#[Object]
impl GqlStatus {
    async fn ok(&self) -> bool {
        self.success
    }

    async fn message(&self) -> &str {
        &self.message
    }
}

impl GqlStatus {
    fn success(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: msg.into(),
        }
    }
}

struct GqlQueueMutationResult {
    success: bool,
    message: String,
    added_count: i32,
    queue_item_ids: Vec<String>,
}

#[Object]
impl GqlQueueMutationResult {
    async fn ok(&self) -> bool {
        self.success
    }

    async fn message(&self) -> &str {
        &self.message
    }

    async fn added_count(&self) -> i32 {
        self.added_count
    }

    async fn queue_item_ids(&self) -> &[String] {
        &self.queue_item_ids
    }
}

// ---------------------------------------------------------------------------
// DB handle wrapper (so we can put it in Context)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DbHandle {
    path: PathBuf,
}

impl DbHandle {
    fn open(&self) -> async_graphql::Result<Database> {
        Database::open(&self.path)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))
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
            artists.retain(|a| {
                artist_genres(&db, a.id)
                    .iter()
                    .any(|ag| ag.contains(&g_lower))
            });
        }

        if favourites_only {
            let fav_artist_ids = favourite_artist_ids(&db)?;
            artists.retain(|a| fav_artist_ids.contains(&a.id));
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
            albums.retain(|a| {
                album_genres(&db, a.id)
                    .iter()
                    .any(|ag| ag.contains(&g_lower))
            });
        }

        if favourites_only {
            let fav_album_ids = favourite_album_ids(&db)?;
            albums.retain(|a| fav_album_ids.contains(&a.id));
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

    async fn queue(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<GqlQueueEntry>> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let (items, cursor) = state.snapshot_playlist();
        Ok(items
            .iter()
            .map(|item| GqlQueueEntry {
                queue_item_id: item.id.0.to_string(),
                title: item.title.clone(),
                artist: item.artist.clone(),
                album: item.album.clone(),
                codec: item.codec.clone(),
                track_number: item.track_number,
                disc: item.disc,
                duration_ms: item.duration_ms,
                is_current: cursor == Some(item.id),
            })
            .collect())
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
        let devices = device::list_output_devices()
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
}

// ---------------------------------------------------------------------------
// Mutation root
// ---------------------------------------------------------------------------

pub struct MutationRoot;

fn parse_queue_item_id(s: &str) -> async_graphql::Result<QueueItemId> {
    Uuid::parse_str(s)
        .map(QueueItemId)
        .map_err(|e| async_graphql::Error::new(format!("invalid queue item ID '{}': {}", s, e)))
}

fn send_cmd(ctx: &Context<'_>, cmd: PlayerCommand) -> async_graphql::Result<()> {
    let tx = ctx.data::<Sender<PlayerCommand>>()?;
    tx.send(cmd)
        .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))
}

#[Object]
impl MutationRoot {
    // -- Playback --

    async fn play(
        &self,
        ctx: &Context<'_>,
        queue_item_id: String,
    ) -> async_graphql::Result<GqlStatus> {
        let id = parse_queue_item_id(&queue_item_id)?;
        send_cmd(ctx, PlayerCommand::Play(id))?;
        Ok(GqlStatus::success("playing"))
    }

    async fn pause(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Pause)?;
        Ok(GqlStatus::success("paused"))
    }

    async fn resume(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Resume)?;
        Ok(GqlStatus::success("resumed"))
    }

    async fn stop(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Stop)?;
        Ok(GqlStatus::success("stopped"))
    }

    async fn next(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::NextTrack)?;
        Ok(GqlStatus::success("skipped to next"))
    }

    async fn previous(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::PrevTrack)?;
        Ok(GqlStatus::success("skipped to previous"))
    }

    async fn seek(&self, ctx: &Context<'_>, position_ms: i64) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Seek(position_ms as u64))?;
        Ok(GqlStatus::success(format!("seeked to {}ms", position_ms)))
    }

    // -- Queue --

    async fn add_to_queue(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let mut items = Vec::new();
        let mut queue_item_ids = Vec::new();
        for &tid in &track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid) {
                let item = track_to_playlist_item(&track, &db);
                queue_item_ids.push(item.id.0.to_string());
                items.push(item);
            }
        }

        let count = items.len() as i32;
        if !items.is_empty() {
            tx.send(PlayerCommand::AddToPlaylist(items))
                .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

            // Auto-play if stopped
            if state.playback_state() == PlaybackState::Stopped
                && let Some(first_id) = queue_item_ids.first()
                && let Ok(id) = Uuid::parse_str(first_id).map(QueueItemId)
            {
                let _ = tx.send(PlayerCommand::Play(id));
            }
        }

        Ok(GqlQueueMutationResult {
            success: true,
            message: format!("queued {} tracks", count),
            added_count: count,
            queue_item_ids,
        })
    }

    async fn replace_queue(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
    ) -> async_graphql::Result<GqlQueueMutationResult> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        tx.send(PlayerCommand::ClearPlaylist)
            .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

        let mut items = Vec::new();
        let mut queue_item_ids = Vec::new();
        for &tid in &track_ids {
            if let Ok(Some(track)) = queries::get_track_row(&db.conn, tid) {
                let item = track_to_playlist_item(&track, &db);
                queue_item_ids.push(item.id.0.to_string());
                items.push(item);
            }
        }

        let count = items.len() as i32;
        let first_id = items.first().map(|i| i.id);
        if !items.is_empty() {
            tx.send(PlayerCommand::AddToPlaylist(items))
                .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

            if let Some(id) = first_id {
                let _ = tx.send(PlayerCommand::Play(id));
            }
        }

        Ok(GqlQueueMutationResult {
            success: true,
            message: format!("replaced queue with {} tracks", count),
            added_count: count,
            queue_item_ids,
        })
    }

    async fn remove_from_queue(
        &self,
        ctx: &Context<'_>,
        queue_item_ids: Vec<String>,
    ) -> async_graphql::Result<GqlStatus> {
        let ids: Vec<QueueItemId> = queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let count = ids.len();
        send_cmd(ctx, PlayerCommand::RemoveFromPlaylistBatch(ids))?;
        Ok(GqlStatus::success(format!(
            "removed {} items from queue",
            count
        )))
    }

    async fn move_in_queue(
        &self,
        ctx: &Context<'_>,
        queue_item_ids: Vec<String>,
        target_queue_item_id: String,
        after: bool,
    ) -> async_graphql::Result<GqlStatus> {
        let ids: Vec<QueueItemId> = queue_item_ids
            .iter()
            .map(|s| parse_queue_item_id(s))
            .collect::<Result<Vec<_>, _>>()?;
        let target = parse_queue_item_id(&target_queue_item_id)?;
        send_cmd(
            ctx,
            PlayerCommand::MoveItemsInPlaylist { ids, target, after },
        )?;
        Ok(GqlStatus::success("queue reordered"))
    }

    async fn clear_queue(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::ClearPlaylist)?;
        Ok(GqlStatus::success("queue cleared"))
    }

    async fn undo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Undo)?;
        Ok(GqlStatus::success("undone"))
    }

    async fn redo(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::Redo)?;
        Ok(GqlStatus::success("redone"))
    }

    // -- Device --

    async fn set_device(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::SetOutputDevice(name.clone()))?;
        Ok(GqlStatus::success(format!("switched to device '{}'", name)))
    }

    async fn clear_device(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        send_cmd(ctx, PlayerCommand::ClearOutputDevice)?;
        Ok(GqlStatus::success("device cleared, using system default"))
    }

    // -- Favourites --

    async fn favourite(&self, ctx: &Context<'_>, track_id: i64) -> async_graphql::Result<GqlTrack> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;
        queries::add_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        sync_favourite_to_remote(&db, path, true);
        Ok(GqlTrack { row: track })
    }

    async fn unfavourite(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<GqlTrack> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;
        queries::remove_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        sync_favourite_to_remote(&db, path, false);
        Ok(GqlTrack { row: track })
    }

    async fn toggle_favourite(
        &self,
        ctx: &Context<'_>,
        track_id: i64,
    ) -> async_graphql::Result<GqlTrack> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let track = queries::get_track_row(&db.conn, track_id)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", track_id)))?;
        let path = track
            .path
            .as_ref()
            .or(track.cached_path.as_ref())
            .ok_or_else(|| async_graphql::Error::new(format!("track {} has no path", track_id)))?;
        let is_now_fav = queries::toggle_favourite(&db.conn, std::path::Path::new(path))
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        sync_favourite_to_remote(&db, path, is_now_fav);
        Ok(GqlTrack { row: track })
    }

    // -- Snapshots --

    async fn save_snapshot(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        let (items, cursor) = state.snapshot_playlist();
        let position_ms = state.position_ms();

        let persisted: Vec<PersistedQueueItem> = items
            .iter()
            .map(PersistedQueueItem::from_playlist_item)
            .collect();
        let cursor_path = cursor.and_then(|cid| {
            items
                .iter()
                .find(|i| i.id == cid)
                .map(|i| i.path.to_string_lossy().into_owned())
        });

        queries::save_snapshot(
            &db.conn,
            &name,
            &persisted,
            cursor_path.as_deref(),
            position_ms,
        )
        .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;

        Ok(GqlStatus::success(format!("saved snapshot '{}'", name)))
    }

    async fn restore_snapshot(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let tx = ctx.data::<Sender<PlayerCommand>>()?;

        let snap = queries::load_snapshot(&db.conn, &name)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
            .ok_or_else(|| async_graphql::Error::new(format!("snapshot '{}' not found", name)))?;

        let items: Vec<koan_core::player::state::PlaylistItem> =
            snap.items.iter().map(|i| i.to_playlist_item()).collect();

        // Clear + add + play the cursor track
        tx.send(PlayerCommand::ClearPlaylist)
            .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;

        let cursor_item_id = snap.cursor_path.as_ref().and_then(|cp| {
            items
                .iter()
                .find(|i| i.path.to_string_lossy() == cp.as_str())
                .map(|i| i.id)
        });

        if !items.is_empty() {
            let first_id = cursor_item_id.unwrap_or(items[0].id);
            tx.send(PlayerCommand::AddToPlaylist(items))
                .map_err(|e| async_graphql::Error::new(format!("send error: {}", e)))?;
            let _ = tx.send(PlayerCommand::Play(first_id));
            if snap.position_ms > 0 {
                let _ = tx.send(PlayerCommand::Seek(snap.position_ms));
            }
        }

        Ok(GqlStatus::success(format!("restored snapshot '{}'", name)))
    }

    async fn delete_snapshot(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<GqlStatus> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let deleted = queries::delete_snapshot(&db.conn, &name)
            .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?;
        if deleted {
            Ok(GqlStatus::success(format!("deleted snapshot '{}'", name)))
        } else {
            Err(async_graphql::Error::new(format!(
                "snapshot '{}' not found",
                name
            )))
        }
    }

    // -- Sharing --

    async fn create_share(
        &self,
        ctx: &Context<'_>,
        track_ids: Vec<i64>,
        description: Option<String>,
    ) -> async_graphql::Result<GqlShare> {
        let db = ctx.data::<DbHandle>()?.open()?;
        let mut remote_ids = Vec::new();
        for &tid in &track_ids {
            let track = queries::get_track_row(&db.conn, tid)
                .map_err(|e| async_graphql::Error::new(format!("db error: {}", e)))?
                .ok_or_else(|| async_graphql::Error::new(format!("track {} not found", tid)))?;
            let rid = track.remote_id.ok_or_else(|| {
                async_graphql::Error::new(format!("track {} has no remote_id", tid))
            })?;
            remote_ids.push(rid);
        }

        let cfg = Config::load().unwrap_or_default();
        if !cfg.remote.enabled {
            return Err(async_graphql::Error::new("remote server not configured"));
        }
        let password = super::get_remote_password(&cfg);
        let client = koan_core::remote::client::SubsonicClient::new(
            &cfg.remote.url,
            &cfg.remote.username,
            &password,
        );

        let id_refs: Vec<&str> = remote_ids.iter().map(|s| s.as_str()).collect();
        let share = client
            .create_share(&id_refs, description.as_deref())
            .map_err(|e| async_graphql::Error::new(format!("remote error: {}", e)))?;

        Ok(GqlShare {
            id: share.id,
            url: share.url,
            description: share.description,
            expires: share.expires,
            visit_count: share.visit_count.unwrap_or(0),
        })
    }

    // -- Radio --

    async fn enable_radio(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        state.set_radio_mode(true);
        Ok(GqlStatus::success("radio mode enabled"))
    }

    async fn disable_radio(&self, ctx: &Context<'_>) -> async_graphql::Result<GqlStatus> {
        let state = ctx.data::<Arc<SharedPlayerState>>()?;
        state.set_radio_mode(false);
        Ok(GqlStatus::success("radio mode disabled"))
    }
}

// ---------------------------------------------------------------------------
// Helper: year extraction from date strings ("2024", "2024-01-15", etc)
// ---------------------------------------------------------------------------

fn extract_year(date: &str) -> Option<i32> {
    date.get(..4).and_then(|s| s.parse().ok())
}

/// Get album year from its date field.
fn album_year(album: &queries::AlbumRow) -> Option<i32> {
    album.date.as_deref().and_then(extract_year)
}

/// Get genres for an artist (distinct genres from their tracks).
fn artist_genres(db: &Database, artist_id: i64) -> HashSet<String> {
    queries::tracks_for_artist(&db.conn, artist_id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| t.genre)
        .map(|g| g.to_lowercase())
        .collect()
}

/// Get genres for an album (distinct genres from its tracks).
fn album_genres(db: &Database, album_id: i64) -> HashSet<String> {
    queries::tracks_for_album(&db.conn, album_id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| t.genre)
        .map(|g| g.to_lowercase())
        .collect()
}

/// Get the year for a track via its album's date.
fn track_year(db: &Database, track: &queries::TrackRow) -> Option<i32> {
    track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten())
        .as_deref()
        .and_then(extract_year)
}

// ---------------------------------------------------------------------------
// Helper: favourite ID sets for filtering
// ---------------------------------------------------------------------------

/// Get the set of artist IDs that have at least one favourited track.
fn favourite_artist_ids(db: &Database) -> async_graphql::Result<HashSet<i64>> {
    let fav_paths =
        queries::load_favourites(&db.conn).map_err(|e| async_graphql::Error::new(e.to_string()))?;
    let mut ids = HashSet::new();
    for path in &fav_paths {
        let path_str = path.to_string_lossy();
        if let Ok(Some(tid)) = queries::track_id_by_path(&db.conn, &path_str)
            && let Ok(Some(row)) = queries::get_track_row(&db.conn, tid)
            && let Some(aid) = row.artist_id
        {
            ids.insert(aid);
        }
    }
    Ok(ids)
}

/// Get the set of album IDs that have at least one favourited track.
fn favourite_album_ids(db: &Database) -> async_graphql::Result<HashSet<i64>> {
    let fav_paths =
        queries::load_favourites(&db.conn).map_err(|e| async_graphql::Error::new(e.to_string()))?;
    let mut ids = HashSet::new();
    for path in &fav_paths {
        let path_str = path.to_string_lossy();
        if let Ok(Some(tid)) = queries::track_id_by_path(&db.conn, &path_str)
            && let Ok(Some(row)) = queries::get_track_row(&db.conn, tid)
            && let Some(aid) = row.album_id
        {
            ids.insert(aid);
        }
    }
    Ok(ids)
}

fn sync_favourite_to_remote(db: &Database, path: &str, star: bool) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let remote_id = queries::remote_id_for_path(&db.conn, std::path::Path::new(path))
        .ok()
        .flatten();
    if let Some(rid) = remote_id {
        let password = super::get_remote_password(&cfg);
        let client = koan_core::remote::client::SubsonicClient::new(
            &cfg.remote.url,
            &cfg.remote.username,
            &password,
        );
        std::thread::Builder::new()
            .name("koan-fav-sync".into())
            .spawn(move || {
                let result = if star {
                    client.star(&rid)
                } else {
                    client.unstar(&rid)
                };
                if let Err(e) = result {
                    log::warn!("failed to sync favourite to remote: {}", e);
                }
            })
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Helper: TrackRow -> PlaylistItem (for queue mutations via GraphQL)
// ---------------------------------------------------------------------------

pub fn track_to_playlist_item(
    track: &queries::TrackRow,
    db: &Database,
) -> koan_core::player::state::PlaylistItem {
    use koan_core::player::state::{LoadState, PlaylistItem};

    let album_date = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let (path, load_state) = if let Some(ref p) = track.path {
        (std::path::PathBuf::from(p), LoadState::Ready)
    } else if let Some(ref cp) = track.cached_path {
        (std::path::PathBuf::from(cp), LoadState::Ready)
    } else {
        (
            std::path::PathBuf::from(format!("/tmp/koan-pending-{}", track.id)),
            LoadState::Pending,
        )
    };

    let year = album_date.as_deref().and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });

    PlaylistItem {
        id: QueueItemId::new(),
        path,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        load_state,
    }
}

// ---------------------------------------------------------------------------
// Schema builder
// ---------------------------------------------------------------------------

pub type KoanSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

pub fn build_schema(
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    db_path: PathBuf,
) -> KoanSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(DbHandle { path: db_path })
        .data(state)
        .data(cmd_tx)
        .finish()
}

// ---------------------------------------------------------------------------
// `koan graphql` entry point
// ---------------------------------------------------------------------------

pub fn cmd_graphql(port: Option<u16>, playground: bool) {
    use axum::routing::{get, post};
    use koan_core::player::Player;

    let _db = open_db();
    let db_path = koan_core::config::db_path();

    let cfg = Config::load().unwrap_or_default();
    let port = port.unwrap_or(cfg.graphql.port);
    let playground_enabled = playground || cfg.graphql.playground;

    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    let schema = build_schema(state, cmd_tx, db_path);

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let mut app = axum::Router::new().route("/graphql", post(graphql_handler));

        if playground_enabled {
            app = app.route("/graphql", get(graphql_playground));
        }

        let app = app.with_state(schema);

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        eprintln!("koan graphql server listening on http://0.0.0.0:{}", port);
        if playground_enabled {
            eprintln!("  GraphQL Playground: http://localhost:{}/graphql", port);
        }

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("failed to bind");
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .expect("server error");
    });
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    eprintln!("\nshutting down...");
}

async fn graphql_handler(
    axum::extract::State(schema): axum::extract::State<KoanSchema>,
    req: async_graphql_axum::GraphQLRequest,
) -> async_graphql_axum::GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

async fn graphql_playground() -> axum::response::Html<String> {
    axum::response::Html(async_graphql::http::playground_source(
        async_graphql::http::GraphQLPlaygroundConfig::new("/graphql"),
    ))
}

/// Run the GraphQL server as a background daemon.
/// Forks the process, writes the PID to ~/.config/koan/graphql.pid, and exits.
pub fn cmd_graphql_daemon(port: Option<u16>, playground: bool) {
    use std::fs;
    use std::process::Command;

    let cfg = Config::load().unwrap_or_default();
    let port_val = port.unwrap_or(cfg.graphql.port);

    let exe = std::env::current_exe().expect("failed to get current exe path");
    let mut cmd = Command::new(exe);
    cmd.arg("graphql");
    cmd.arg("--port").arg(port_val.to_string());
    if playground || cfg.graphql.playground {
        cmd.arg("--playground");
    }

    // Detach: redirect stdio to /dev/null, start in new session
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd.spawn().expect("failed to spawn daemon process");
    let pid = child.id();

    // Write PID file
    let pid_path = koan_core::config::config_dir().join("graphql.pid");
    fs::write(&pid_path, pid.to_string()).ok();

    // Detach — we don't want to wait, the child is a long-running server.
    // Spawn a thread to reap it so we don't leave a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    eprintln!(
        "koan graphql daemon started (pid {}) on port {}",
        pid, port_val
    );
    eprintln!("  PID file: {}", pid_path.display());
}

// ---------------------------------------------------------------------------
// In-process execution (for MCP `graphql` tool)
// ---------------------------------------------------------------------------

/// Execute a GraphQL query in-process (no HTTP round-trip).
pub async fn execute_in_process(
    schema: &KoanSchema,
    query: &str,
    variables: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut request = async_graphql::Request::new(query);
    if let Some(serde_json::Value::Object(map)) = variables {
        let mut gql_vars = async_graphql::Variables::default();
        for (k, v) in map {
            gql_vars.insert(
                async_graphql::Name::new(&k),
                async_graphql::Value::from_json(v).unwrap_or(async_graphql::Value::Null),
            );
        }
        request = request.variables(gql_vars);
    }
    let response = schema.execute(request).await;
    serde_json::to_value(&response).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::db::connection::Database;
    use koan_core::db::queries;
    use koan_core::player::commands::CommandChannel;
    use tempfile::TempDir;

    fn test_schema() -> (
        KoanSchema,
        crossbeam_channel::Receiver<PlayerCommand>,
        TempDir,
    ) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();

        let state = SharedPlayerState::new();
        let ch = CommandChannel::new();
        let tx = ch.tx.clone();
        let rx = ch.rx.clone();

        let schema = build_schema(state, tx, db_path);
        (schema, rx, tmp)
    }

    fn insert_test_track(db_path: &std::path::Path, title: &str, artist: &str, album: &str) -> i64 {
        let db = Database::open(db_path).unwrap();
        let meta = queries::TrackMeta {
            title: title.to_string(),
            artist: artist.to_string(),
            album_artist: Some(artist.to_string()),
            album: album.to_string(),
            track_number: Some(1),
            disc: Some(1),
            date: Some("2024".into()),
            genre: Some("Electronic".into()),
            duration_ms: Some(240000),
            path: Some(format!(
                "/tmp/test/{}.flac",
                title.to_lowercase().replace(' ', "_")
            )),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1411),
            size_bytes: Some(42_000_000),
            mtime: Some(1700000000),
            source: "local".into(),
            remote_id: None,
            remote_url: None,
            label: None,
        };
        queries::upsert_track(&db.conn, &meta).unwrap()
    }

    #[test]
    fn schema_builds() {
        let (_schema, _rx, _tmp) = test_schema();
    }

    #[tokio::test]
    async fn library_stats_query() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "Track1", "Artist1", "Album1");

        let resp = schema
            .execute("{ libraryStats { totalTracks totalAlbums totalArtists } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["libraryStats"]["totalTracks"], 1);
        assert_eq!(data["libraryStats"]["totalAlbums"], 1);
        assert_eq!(data["libraryStats"]["totalArtists"], 1);
    }

    #[tokio::test]
    async fn artists_query() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "T1", "Aphex Twin", "Drukqs");
        insert_test_track(&db_path, "T2", "Boards of Canada", "MHTRTC");

        let resp = schema
            .execute("{ artists { edges { node { id name } } } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let edges = data["artists"]["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 2);
    }

    #[tokio::test]
    async fn tracks_search() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "Windowlicker", "Aphex Twin", "Windowlicker EP");
        insert_test_track(&db_path, "Roygbiv", "Boards of Canada", "MHTRTC");

        let resp = schema
            .execute(r#"{ tracks(search: "Aphex") { edges { node { id title artist } } } }"#)
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let edges = data["tracks"]["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["node"]["title"], "Windowlicker");
    }

    #[tokio::test]
    async fn now_playing_stopped() {
        let (schema, _rx, _tmp) = test_schema();
        let resp = schema
            .execute("{ nowPlaying { state positionMs track { title } } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["nowPlaying"]["state"], "STOPPED");
    }

    #[tokio::test]
    async fn pause_mutation() {
        let (schema, rx, _tmp) = test_schema();
        let resp = schema.execute("mutation { pause { ok message } }").await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["pause"]["ok"], true);
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::Pause));
    }

    #[tokio::test]
    async fn nested_artist_albums_tracks() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "Vordhosbn", "Aphex Twin", "Drukqs");
        insert_test_track(&db_path, "Avril 14th", "Aphex Twin", "Drukqs");

        let resp = schema
            .execute(
                r#"{ artists(search: "Aphex") {
                    edges { node {
                        name
                        albums { edges { node {
                            title
                            tracks { edges { node { title } } }
                        } } }
                    } }
                } }"#,
            )
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let artist = &data["artists"]["edges"][0]["node"];
        assert_eq!(artist["name"], "Aphex Twin");
        let album = &artist["albums"]["edges"][0]["node"];
        assert_eq!(album["title"], "Drukqs");
        let tracks = album["tracks"]["edges"].as_array().unwrap();
        assert_eq!(tracks.len(), 2);
    }

    #[tokio::test]
    async fn pagination_has_next() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        for i in 0..5 {
            insert_test_track(
                &db_path,
                &format!("Track{}", i),
                "Artist",
                &format!("Album{}", i),
            );
        }

        let resp = schema
            .execute(
                r#"{ artists(first: 1) {
                    edges { node { name } cursor }
                    pageInfo { hasNextPage endCursor }
                } }"#,
            )
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        // Only 1 artist ("Artist"), so hasNextPage should be false
        // since all 5 tracks are by the same artist.
        assert_eq!(data["artists"]["edges"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn clear_queue_mutation() {
        let (schema, rx, _tmp) = test_schema();
        let resp = schema
            .execute("mutation { clearQueue { ok message } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PlayerCommand::ClearPlaylist));
    }

    #[tokio::test]
    async fn create_share_track_not_found() {
        let (schema, _rx, _tmp) = test_schema();
        let resp = schema
            .execute(r#"mutation { createShare(trackIds: [9999]) { id url } }"#)
            .await;
        assert!(!resp.errors.is_empty(), "should error for missing track");
        assert!(
            resp.errors[0].message.contains("not found"),
            "error: {}",
            resp.errors[0].message,
        );
    }

    #[tokio::test]
    async fn create_share_no_remote_id() {
        let (schema, _rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");
        let tid = insert_test_track(&db_path, "LocalOnly", "Artist", "Album");

        let query = format!(
            r#"mutation {{ createShare(trackIds: [{}]) {{ id url }} }}"#,
            tid,
        );
        let resp = schema.execute(&query).await;
        assert!(!resp.errors.is_empty(), "should error for local-only track");
        assert!(
            resp.errors[0].message.contains("no remote_id"),
            "error: {}",
            resp.errors[0].message,
        );
    }
}

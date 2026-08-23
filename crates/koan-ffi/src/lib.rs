//! In-process bindings for native GUI clients.
//!
//! This is the same facade `koan-server` puts behind GraphQL, minus the wire.
//! Every method is "read `SharedPlayerState` / hit the DB / send a
//! `PlayerCommand`" — the mapping koan-core's helpers already own. A local app
//! sits on top of the audio engine, so it has no business round-tripping HTTP
//! to reach it; GraphQL stays the surface for clients that genuinely can't
//! link the core (web, iOS, jukebox remotes).
//!
//! Threading: the engine is `Send + Sync` and every method blocks. Callers on
//! the UI thread should keep the long ones (`scan`, `fuzzy_search` over a large
//! library) on a background queue. DB connections are opened per call, matching
//! what the GraphQL resolvers do — WAL makes that cheap and it sidesteps
//! holding a lock across a scan.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::config::{self, Config};
use koan_core::db::connection::Database;
use koan_core::db::queries::{self, PersistedQueueItem};
use koan_core::helpers::spawn_downloads;
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    LoadState, PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState,
};
use uuid::Uuid;

mod types;
pub use types::*;

uniffi::setup_scaffolding!();

/// What `fuzzy_search` matches against.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Track,
    Album,
    Artist,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct FuzzyMatch {
    pub id: i64,
    /// Pre-joined display text — the same string that was matched against.
    pub name: String,
    pub kind: SearchKind,
}

/// Events pushed from the engine, so clients don't have to poll.
///
/// Watching happens on a Rust thread reading atomics, not from the audio or
/// decode threads: `set_position_ms` is a hot-path store and calling into a
/// foreign language from there would put an unbounded amount of work in a
/// timing-sensitive path. Discrete changes fire as they happen; position ticks
/// only while something is playing, and only when the value actually moved.
#[uniffi::export(with_foreign)]
pub trait PlayerEvents: Send + Sync {
    /// State, track, or format changed — anything a transport bar displays
    /// other than the position.
    fn playback_changed(&self, now_playing: NowPlaying);
    /// The queue was mutated. Carries the version so a client can skip a
    /// refetch it has already done.
    fn queue_changed(&self, version: u64);
    /// Playback position, while playing.
    fn position_changed(&self, position_ms: u64);
}

/// The player, the library, and the bridge between them.
#[derive(uniffi::Object)]
pub struct KoanEngine {
    state: Arc<SharedPlayerState>,
    tx: Sender<PlayerCommand>,
    db_path: PathBuf,
    listener: Arc<parking_lot::RwLock<Option<Arc<dyn PlayerEvents>>>>,
}

#[uniffi::export]
impl KoanEngine {
    /// Spawns the player thread and opens the library. One per process.
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, KoanError> {
        let db_path = config::db_path();
        // Fail fast on a broken library rather than after the audio threads exist.
        Database::open(&db_path).map_err(|e| KoanError::Database {
            message: e.to_string(),
        })?;

        let (state, _timeline, _viz, tx) = Player::spawn();
        koan_core::radio::spawn_autoqueue(state.clone(), tx.clone(), db_path.clone());

        let listener: Arc<parking_lot::RwLock<Option<Arc<dyn PlayerEvents>>>> =
            Arc::new(parking_lot::RwLock::new(None));
        let engine = Arc::new(Self {
            state,
            tx,
            db_path,
            listener,
        });
        engine.spawn_watcher();
        Ok(engine)
    }

    // --- Transport ---------------------------------------------------------

    /// Move the cursor to `queue_item_id` and start playing it.
    pub fn play(&self, queue_item_id: String) -> Result<(), KoanError> {
        self.send(PlayerCommand::Play(parse_qid(&queue_item_id)?))
    }

    pub fn pause(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::Pause)
    }

    pub fn resume(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::Resume)
    }

    pub fn stop(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::Stop)
    }

    /// Space-bar behaviour: pause when playing, resume otherwise.
    pub fn toggle_play_pause(&self) -> Result<(), KoanError> {
        match self.state.playback_state() {
            PlaybackState::Playing => self.pause(),
            _ => self.resume(),
        }
    }

    pub fn next(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::NextTrack)
    }

    pub fn previous(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::PrevTrack)
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), KoanError> {
        self.send(PlayerCommand::Seek(position_ms))
    }

    // --- Observable state --------------------------------------------------

    /// One consistent read of everything the transport bar needs. The UI polls
    /// this; `playlist_version` tells it whether the queue also needs refetching.
    pub fn now_playing(&self) -> NowPlaying {
        let info = self.state.track_info();
        let cursor = self.state.cursor();
        let play_state = self.state.playback_state();
        let entry = cursor
            .and_then(|cid| self.state.get_item(cid))
            .map(|item| QueueItem::from_cursor_item(&item, play_state));

        NowPlaying {
            state: play_state.into(),
            position_ms: self.state.position_ms(),
            duration_ms: info.as_ref().map(|i| i.duration_ms).unwrap_or(0),
            queue_item_id: cursor.map(|c| c.0.to_string()),
            entry,
            format: info.as_ref().map(StreamFormat::from),
            playlist_version: self.state.playlist_version(),
            radio_enabled: self.state.radio_mode(),
        }
    }

    /// Start pushing events to `listener`. Replaces polling `now_playing()`.
    ///
    /// One listener at a time — a second call replaces the first, which is what
    /// a single UI wants and avoids leaking a listener across a reload.
    pub fn subscribe(&self, listener: Arc<dyn PlayerEvents>) {
        *self.listener.write() = Some(listener);
    }

    pub fn unsubscribe(&self) {
        *self.listener.write() = None;
    }

    /// Cheap enough to poll every frame — use it to decide whether to call
    /// `queue()`, which allocates the whole list.
    pub fn playlist_version(&self) -> u64 {
        self.state.playlist_version()
    }

    pub fn queue(&self) -> Vec<QueueItem> {
        self.state
            .derive_visible_queue()
            .entries
            .iter()
            .map(QueueItem::from)
            .collect()
    }

    // --- Queue mutation ----------------------------------------------------

    /// Append tracks. Starts playback if the player was stopped, and kicks off
    /// downloads for anything remote. Returns the new queue item IDs.
    pub fn add_to_queue(&self, track_ids: Vec<i64>) -> Result<Vec<String>, KoanError> {
        let db = self.db()?;
        let (items, pending) = self.build_items(&db, &track_ids);
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<String> = items.iter().map(|i| i.id.0.to_string()).collect();
        let first = items[0].id;
        let was_stopped = self.state.playback_state() == PlaybackState::Stopped;

        self.send(PlayerCommand::AddToPlaylist(items))?;
        if was_stopped {
            let _ = self.tx.send(PlayerCommand::Play(first));
        }
        self.start_downloads(pending);

        Ok(ids)
    }

    /// Clear the queue and play `track_ids` from the top.
    /// Replace the queue, starting at `start_at` (default: the first track).
    ///
    /// The index is part of the command rather than a follow-up `play` because
    /// two commands means the first track audibly starts before the jump lands:
    /// clicking track nine of an album flashed track one as playing first.
    /// An index past the end starts at the beginning.
    pub fn replace_queue(
        &self,
        track_ids: Vec<i64>,
        start_at: Option<u32>,
    ) -> Result<Vec<String>, KoanError> {
        let db = self.db()?;
        let (items, pending) = self.build_items(&db, &track_ids);
        if items.is_empty() {
            self.send(PlayerCommand::ClearPlaylist)?;
            return Ok(Vec::new());
        }

        let ids: Vec<String> = items.iter().map(|i| i.id.0.to_string()).collect();
        self.send(PlayerCommand::ReplacePlaylist {
            items,
            start: start_at.unwrap_or(0) as usize,
        })?;
        self.start_downloads(pending);

        Ok(ids)
    }

    /// Insert after an existing item — what a drop between two rows means.
    pub fn insert_after(
        &self,
        track_ids: Vec<i64>,
        after_queue_item_id: String,
    ) -> Result<Vec<String>, KoanError> {
        let after = parse_qid(&after_queue_item_id)?;
        let db = self.db()?;
        let (items, pending) = self.build_items(&db, &track_ids);
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<String> = items.iter().map(|i| i.id.0.to_string()).collect();
        self.send(PlayerCommand::InsertInPlaylist { items, after })?;
        self.start_downloads(pending);

        Ok(ids)
    }

    /// Removed as one undo step, however many IDs are passed.
    pub fn remove_from_queue(&self, queue_item_ids: Vec<String>) -> Result<(), KoanError> {
        let ids = parse_qids(&queue_item_ids)?;
        self.send(PlayerCommand::RemoveFromPlaylistBatch(ids))
    }

    /// Reorder. `after` puts the items below the target rather than above.
    pub fn move_in_queue(
        &self,
        queue_item_ids: Vec<String>,
        target_queue_item_id: String,
        after: bool,
    ) -> Result<(), KoanError> {
        let ids = parse_qids(&queue_item_ids)?;
        let target = parse_qid(&target_queue_item_id)?;
        self.send(PlayerCommand::MoveItemsInPlaylist { ids, target, after })
    }

    pub fn clear_queue(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::ClearPlaylist)
    }

    pub fn undo(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::Undo)
    }

    pub fn redo(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::Redo)
    }

    // --- Library -----------------------------------------------------------

    pub fn artists(&self, search: Option<String>) -> Result<Vec<Artist>, KoanError> {
        let db = self.db()?;
        let rows = match search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(q) => queries::find_artists(&db.conn, q),
            None => queries::all_artists(&db.conn),
        }
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Artist::from).collect())
    }

    pub fn albums(&self, artist_id: Option<i64>, sort: AlbumSort) -> Result<Vec<Album>, KoanError> {
        let db = self.db()?;
        let rows = match artist_id {
            Some(id) => queries::albums_for_artist(&db.conn, id),
            None => queries::all_albums(&db.conn),
        }
        .map_err(db_err)?;

        let mut albums: Vec<Album> = rows.into_iter().map(Album::from).collect();
        match sort {
            // Albums predating the added_at column sort last rather than first,
            // which is what an empty string would do.
            AlbumSort::RecentlyAdded => albums.sort_by(|a, b| {
                b.added_at
                    .as_deref()
                    .unwrap_or("")
                    .cmp(a.added_at.as_deref().unwrap_or(""))
            }),
            AlbumSort::Title => albums.sort_by_key(|a| a.title.to_lowercase()),
            AlbumSort::Artist => {
                albums.sort_by_key(|a| (a.artist_name.to_lowercase(), a.year.unwrap_or(0)))
            }
            AlbumSort::Year => albums.sort_by_key(|a| std::cmp::Reverse(a.year.unwrap_or(0))),
            AlbumSort::Random => shuffle(&mut albums),
        }
        Ok(albums)
    }

    pub fn album(&self, album_id: i64) -> Result<Option<Album>, KoanError> {
        let db = self.db()?;
        Ok(queries::get_album(&db.conn, album_id)
            .map_err(db_err)?
            .map(Album::from))
    }

    /// Tracks for an album or artist, or a page of the whole library when
    /// neither is given.
    pub fn tracks(
        &self,
        album_id: Option<i64>,
        artist_id: Option<i64>,
        sort: TrackSort,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Track>, KoanError> {
        let db = self.db()?;
        let rows = match (album_id, artist_id) {
            (Some(aid), _) => queries::tracks_for_album(&db.conn, aid),
            (None, Some(aid)) => queries::tracks_for_artist(&db.conn, aid),
            (None, None) => queries::all_tracks_paged(&db.conn, limit, offset),
        }
        .map_err(db_err)?;
        Ok(self.decorate(&db, sort_rows(rows, sort)))
    }

    pub fn track(&self, track_id: i64) -> Result<Option<Track>, KoanError> {
        let db = self.db()?;
        let Some(row) = queries::get_track_row(&db.conn, track_id).map_err(db_err)? else {
            return Ok(None);
        };
        Ok(self.decorate(&db, vec![row]).into_iter().next())
    }

    /// FTS5 search across title, artist, album, genre.
    pub fn search(&self, query: String, limit: u32) -> Result<Vec<Track>, KoanError> {
        let db = self.db()?;
        let rows = queries::search_tracks_paged(&db.conn, &query, limit, 0).map_err(db_err)?;
        Ok(self.decorate(&db, rows))
    }

    /// Nucleo fuzzy match — what the command palette wants. Ranked, best first.
    pub fn fuzzy_search(
        &self,
        query: String,
        kind: SearchKind,
        limit: u32,
    ) -> Result<Vec<FuzzyMatch>, KoanError> {
        use nucleo::pattern::{CaseMatching, Normalization};
        use nucleo::{Config as NucleoConfig, Nucleo};

        let db = self.db()?;
        let items: Vec<(i64, String)> = match kind {
            SearchKind::Track => queries::all_tracks(&db.conn)
                .map_err(db_err)?
                .into_iter()
                .map(|t| {
                    (
                        t.id,
                        format!("{} — {} — {}", t.artist_name, t.album_title, t.title),
                    )
                })
                .collect(),
            SearchKind::Album => queries::all_albums(&db.conn)
                .map_err(db_err)?
                .into_iter()
                .map(|a| (a.id, format!("{} — {}", a.artist_name, a.title)))
                .collect(),
            SearchKind::Artist => queries::all_artists(&db.conn)
                .map_err(db_err)?
                .into_iter()
                .map(|a| (a.id, a.name))
                .collect(),
        };

        let mut nucleo: Nucleo<u32> = Nucleo::new(NucleoConfig::DEFAULT, Arc::new(|| {}), None, 1);
        let injector = nucleo.injector();
        for (i, (_, text)) in items.iter().enumerate() {
            let text = text.clone();
            injector.push(i as u32, |_val, cols| {
                cols[0] = text.into();
            });
        }

        nucleo
            .pattern
            .reparse(0, &query, CaseMatching::Smart, Normalization::Smart, false);
        for _ in 0..20 {
            nucleo.tick(10);
        }

        let snap = nucleo.snapshot();
        let count = (snap.matched_item_count() as usize).min(limit as usize);
        let mut out = Vec::with_capacity(count);
        for i in 0..count as u32 {
            if let Some(item) = snap.get_matched_item(i)
                && let Some((id, name)) = items.get(*item.data as usize)
            {
                out.push(FuzzyMatch {
                    id: *id,
                    name: name.clone(),
                    kind,
                });
            }
        }
        Ok(out)
    }

    pub fn random_tracks(
        &self,
        count: u32,
        artist_id: Option<i64>,
    ) -> Result<Vec<Track>, KoanError> {
        let db = self.db()?;
        let rows = queries::random_tracks(&db.conn, count, artist_id).map_err(db_err)?;
        Ok(self.decorate(&db, rows))
    }

    pub fn similar_artists(&self, artist_id: i64) -> Result<Vec<SimilarArtist>, KoanError> {
        let db = self.db()?;
        let rows = queries::get_similar_artists_detailed(&db.conn, artist_id).map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|e| SimilarArtist {
                artist_id: e.artist.id,
                name: e.artist.name,
                score: e.score,
                source: e.source,
            })
            .collect())
    }

    pub fn library_stats(&self) -> Result<Stats, KoanError> {
        let db = self.db()?;
        Ok(queries::library_stats(&db.conn).map_err(db_err)?.into())
    }

    /// Raw image bytes — no base64 round trip, unlike the GraphQL surface,
    /// which has to encode because JSON can't carry binary.
    ///
    /// Embedded tags first, then the remote server. A library synced from
    /// Navidrome has no local files to read art out of, so without the remote
    /// fallback every album is blank. `size` requests a thumbnail; the grid
    /// wants one, the now-playing pane doesn't. Network on the remote path —
    /// call it off the main thread.
    pub fn cover_art(
        &self,
        track_id: i64,
        size: Option<u32>,
    ) -> Result<Option<CoverArt>, KoanError> {
        let db = self.db()?;
        let Some(row) = queries::get_track_row(&db.conn, track_id).map_err(db_err)? else {
            return Ok(None);
        };

        if let Some(path) = row.path.as_ref().or(row.cached_path.as_ref())
            && let Some(data) = koan_core::index::metadata::extract_cover_art(Path::new(path))
        {
            let mime = sniff_mime(&data).to_string();
            return Ok(Some(CoverArt { data, mime }));
        }

        let Some(remote_id) = row.remote_id else {
            return Ok(None);
        };
        let cfg = Config::load().unwrap_or_default();
        if !cfg.remote.enabled {
            return Ok(None);
        }
        let Some(client) = koan_core::helpers::subsonic_client(&cfg) else {
            return Ok(None);
        };
        match client.get_cover_art(&remote_id, size) {
            Ok(data) if !data.is_empty() => {
                let mime = sniff_mime(&data).to_string();
                Ok(Some(CoverArt { data, mime }))
            }
            // No art on the server is normal, not a failure worth surfacing.
            _ => Ok(None),
        }
    }

    /// Cached lyrics only — this never hits the network, so it is safe to call
    /// from a view body's task without stalling on LRCLIB.
    pub fn lyrics(&self, track_id: i64) -> Result<Option<Lyrics>, KoanError> {
        let db = self.db()?;
        Ok(queries::get_cached_lyrics(&db.conn, track_id)
            .map_err(db_err)?
            .map(|(content, synced)| {
                let lines = if synced {
                    koan_core::lyrics::parse_lrc(&content)
                        .into_iter()
                        .map(|l| LyricLine {
                            time_secs: l.time_secs,
                            text: l.text,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                Lyrics {
                    content,
                    synced,
                    source: "cache".into(),
                    lines,
                }
            }))
    }

    /// Cache, then LRCLIB. Hits the network on a miss, so never call this from
    /// the main thread — `lyrics()` is the non-blocking read.
    pub fn fetch_lyrics(&self, track_id: i64) -> Result<Option<Lyrics>, KoanError> {
        let db = self.db()?;
        let Some(row) = queries::get_track_row(&db.conn, track_id).map_err(db_err)? else {
            return Ok(None);
        };
        let duration_secs = row.duration_ms.unwrap_or(0).max(0) as u64 / 1000;
        match koan_core::lyrics::fetch_lyrics(
            &db.conn,
            track_id,
            &row.artist_name,
            &row.title,
            &row.album_title,
            duration_secs,
        ) {
            Ok(l) => Ok(Some(l.into())),
            // A track with no lyrics anywhere is the normal case, not an error.
            Err(_) => Ok(None),
        }
    }

    // --- Favourites --------------------------------------------------------

    pub fn favourites(&self) -> Result<Vec<Track>, KoanError> {
        let db = self.db()?;
        let ids = queries::favourite_track_ids_batch(&db.conn).map_err(db_err)?;
        let mut rows = Vec::new();
        for id in ids {
            if let Ok(Some(row)) = queries::get_track_row(&db.conn, id) {
                rows.push(row);
            }
        }
        rows.sort_by(|a, b| {
            (&a.artist_name, &a.album_title, a.disc, a.track_number).cmp(&(
                &b.artist_name,
                &b.album_title,
                b.disc,
                b.track_number,
            ))
        });
        Ok(self.decorate(&db, rows))
    }

    /// Returns the new state. Syncs to the remote server in the background when
    /// one is configured.
    pub fn toggle_favourite(&self, track_id: i64) -> Result<bool, KoanError> {
        let db = self.db()?;
        let path = queries::track_favourite_key(&db.conn, track_id)
            .map_err(db_err)?
            .ok_or_else(|| KoanError::NotFound {
                message: format!("track {track_id}"),
            })?;

        let now_favourite =
            queries::toggle_favourite(&db.conn, Path::new(&path)).map_err(fav_err)?;
        sync_favourite_to_remote(&db, &path, now_favourite);
        Ok(now_favourite)
    }

    // --- Snapshots ---------------------------------------------------------

    pub fn snapshots(&self) -> Result<Vec<Snapshot>, KoanError> {
        let db = self.db()?;
        Ok(queries::list_snapshots(&db.conn)
            .map_err(fav_err)?
            .into_iter()
            .map(Snapshot::from)
            .collect())
    }

    pub fn save_snapshot(&self, name: String) -> Result<(), KoanError> {
        let db = self.db()?;
        let (items, cursor) = self.state.snapshot_playlist();
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
            self.state.position_ms(),
        )
        .map_err(fav_err)
        .map(|_| ())
    }

    /// Replaces the queue and resumes at the saved position. Items still in the
    /// library are re-resolved so cache paths and downloads stay correct.
    pub fn restore_snapshot(&self, name: String) -> Result<(), KoanError> {
        let db = self.db()?;
        let snap = queries::load_snapshot(&db.conn, &name)
            .map_err(fav_err)?
            .ok_or_else(|| KoanError::NotFound {
                message: format!("snapshot '{name}'"),
            })?;

        let (items, pending) = restore_items(&db, &snap.items);

        self.send(PlayerCommand::ClearPlaylist)?;
        if items.is_empty() {
            return Ok(());
        }

        let cursor = snap
            .cursor_path
            .as_ref()
            .and_then(|cp| {
                items
                    .iter()
                    .find(|i| i.path.to_string_lossy() == cp.as_str())
            })
            .map(|i| i.id)
            .unwrap_or(items[0].id);

        self.send(PlayerCommand::AddToPlaylist(items))?;
        let _ = self.tx.send(PlayerCommand::Play(cursor));
        if snap.position_ms > 0 {
            let _ = self.tx.send(PlayerCommand::Seek(snap.position_ms));
        }
        self.start_downloads(pending);

        Ok(())
    }

    pub fn delete_snapshot(&self, name: String) -> Result<bool, KoanError> {
        let db = self.db()?;
        queries::delete_snapshot(&db.conn, &name).map_err(fav_err)
    }

    // --- Session persistence -----------------------------------------------

    /// Write the queue and position so the next launch can pick them up.
    /// Call it on quit; it is cheap enough to call on a timer too.
    pub fn save_session(&self) -> Result<(), KoanError> {
        let db = self.db()?;
        let (items, cursor) = self.state.snapshot_playlist();
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
        queries::save_playback_state(
            &db.conn,
            &persisted,
            cursor_path.as_deref(),
            self.state.position_ms(),
            self.state.playback_state() == PlaybackState::Playing,
            self.state.radio_mode(),
        )
        .map_err(fav_err)
    }

    /// Restore the queue saved by `save_session`, cursor and position included.
    ///
    /// Resumes only if playback was running when the session was saved: closing
    /// a player mid-track and having it pick up where it left off is the point,
    /// while a player that was paused should stay paused rather than start
    /// making noise at whoever opened it.
    ///
    /// Returns the number of items restored.
    pub fn restore_session(&self) -> Result<u32, KoanError> {
        let db = self.db()?;
        let Some(saved) = queries::load_playback_state(&db.conn).map_err(fav_err)? else {
            return Ok(0);
        };

        // Restored whether or not there is a queue left to play.
        self.state.set_radio_mode(saved.radio_enabled);

        let (items, pending) = restore_items(&db, &saved.items);
        if items.is_empty() {
            return Ok(0);
        }

        let count = items.len() as u32;
        let cursor = saved
            .cursor_path
            .as_ref()
            .and_then(|cp| {
                items
                    .iter()
                    .find(|i| i.path.to_string_lossy() == cp.as_str())
            })
            .map(|i| i.id);

        self.send(PlayerCommand::AddToPlaylist(items))?;
        self.start_downloads(pending);

        if let Some(id) = cursor {
            self.state.set_cursor(Some(id));
            self.park_at(id, saved.position_ms, saved.was_playing);
        }

        Ok(count)
    }

    // --- Output device -----------------------------------------------------

    pub fn devices(&self) -> Result<Vec<Device>, KoanError> {
        let devices = koan_core::audio::list_output_devices().map_err(|e| KoanError::Audio {
            message: e.to_string(),
        })?;
        Ok(devices
            .into_iter()
            .map(|d| Device {
                name: d.name,
                sample_rates: d.sample_rates,
            })
            .collect())
    }

    pub fn set_device(&self, name: String) -> Result<(), KoanError> {
        self.send(PlayerCommand::SetOutputDevice(name))
    }

    pub fn clear_device(&self) -> Result<(), KoanError> {
        self.send(PlayerCommand::ClearOutputDevice)
    }

    /// The device name persisted in config, or `None` for the system default.
    /// Read from config rather than the player so it survives a restart.
    pub fn current_device(&self) -> Option<String> {
        Config::load().unwrap_or_default().playback.output_device
    }

    // --- Radio -------------------------------------------------------------

    /// Radio keeps the queue topped up with tracks chosen by similarity to
    /// what you've been listening to. The picking loop is spawned with the
    /// engine and watches this flag.
    pub fn set_radio(&self, enabled: bool) {
        self.state.set_radio_mode(enabled);
    }

    // --- Library maintenance ----------------------------------------------

    /// Rescans every configured library folder. Blocking and slow — call it off
    /// the main thread.
    pub fn scan(&self, force: bool) -> Result<ScanSummary, KoanError> {
        let db = self.db()?;
        let cfg = Config::load().unwrap_or_default();

        let mut summary = ScanSummary {
            added: 0,
            updated: 0,
            removed: 0,
            skipped: 0,
            errors: Vec::new(),
        };
        // `force_remove` stays off: it lifts the brake that stops a failed mount
        // from deleting the library, which is not a call a GUI button should make.
        let opts = koan_core::index::scanner::ScanOptions {
            force,
            force_remove: false,
        };
        for folder in &cfg.library.folders {
            let r = koan_core::index::scanner::scan_folder(&db, folder, opts, None);
            summary.added += r.added as u32;
            summary.updated += r.updated as u32;
            summary.removed += r.removed as u32;
            summary.skipped += r.skipped as u32;
            summary.errors.extend(
                r.errors
                    .into_iter()
                    .map(|(p, e)| format!("{}: {e}", p.display())),
            );
        }
        Ok(summary)
    }

    /// Pull the remote library into the local database. Long and network-bound
    /// — call it off the main thread. `full` ignores the incremental cursor and
    /// re-walks every album.
    pub fn sync_remote(&self, full: bool) -> Result<SyncSummary, KoanError> {
        let db = self.db()?;
        let cfg = Config::load().unwrap_or_default();
        let client =
            koan_core::helpers::subsonic_client(&cfg).ok_or_else(|| KoanError::BadArgument {
                message: "no remote server configured".into(),
            })?;

        let result = koan_core::remote::sync::sync_library(
            &db,
            &client,
            full,
            &cfg.remote.url,
            &cfg.remote.username,
        )
        .map_err(|e| KoanError::Database {
            message: e.to_string(),
        })?;

        Ok(SyncSummary {
            artists: result.artists_synced as u32,
            albums: result.albums_synced as u32,
            tracks: result.tracks_synced as u32,
            albums_failed: result.albums_failed as u32,
        })
    }

    /// Create a public share link on the remote server for these tracks.
    ///
    /// Only tracks the server knows about can go in it — the link points at the
    /// server, so a local-only file has nothing for it to point at. A mixed
    /// selection shares what it can; `skipped` says how much it left out.
    /// Network-bound; keep it off the main thread.
    pub fn create_share(
        &self,
        track_ids: Vec<i64>,
        description: Option<String>,
    ) -> Result<Share, KoanError> {
        let db = self.db()?;
        let cfg = Config::load().unwrap_or_default();
        koan_core::helpers::create_share(&db, &cfg, &track_ids, description.as_deref())
            .map(|outcome| Share {
                url: outcome.url,
                shared: outcome.shared as u32,
                skipped: outcome.skipped as u32,
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
    }

    /// Track IDs for an album or an artist, in running order. What the context
    /// menu actions resolve to before touching the queue.
    pub fn track_ids(
        &self,
        album_id: Option<i64>,
        artist_id: Option<i64>,
    ) -> Result<Vec<i64>, KoanError> {
        Ok(self
            .tracks(album_id, artist_id, TrackSort::Album, 2000, 0)?
            .into_iter()
            .map(|t| t.id)
            .collect())
    }

    /// Where the library folders point. Shown in settings.
    pub fn library_folders(&self) -> Vec<String> {
        Config::load()
            .unwrap_or_default()
            .library
            .folders
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }
}

// --- Internals -------------------------------------------------------------

impl KoanEngine {
    /// Watch shared state and push changes to the listener.
    ///
    /// Polls, but in Rust and over atomics, which is nothing — and the client
    /// sees events. The alternative, notifying from the player's own mutation
    /// points, would put foreign calls on the decode thread.
    fn spawn_watcher(self: &Arc<Self>) {
        let engine = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("koan-events".into())
            .spawn(move || {
                let mut last_version = u64::MAX;
                let mut last_position = u64::MAX;
                let mut last_signature: Option<(PlaybackState, Option<String>)> = None;
                let mut ticks_since_download_nudge = 0u32;

                loop {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let Some(engine) = engine.upgrade() else {
                        return; // Engine dropped; so is the app.
                    };
                    let Some(listener) = engine.listener.read().clone() else {
                        continue;
                    };

                    let state = engine.state.playback_state();
                    let cursor = engine.state.cursor().map(|c| c.0.to_string());
                    let signature = (state, cursor);
                    if last_signature.as_ref() != Some(&signature) {
                        last_signature = Some(signature);
                        listener.playback_changed(engine.now_playing());
                    }

                    let version = engine.state.playlist_version();
                    if version != last_version {
                        last_version = version;
                        listener.queue_changed(version);
                        ticks_since_download_nudge = 0;
                    } else if !engine.state.pending_downloads().is_empty() {
                        // Download progress moves without bumping the version,
                        // so nothing above would announce it. Once a second is
                        // plenty for a progress bar and keeps the client from
                        // having to poll for the one thing events don't cover.
                        ticks_since_download_nudge += 1;
                        if ticks_since_download_nudge >= 10 {
                            ticks_since_download_nudge = 0;
                            listener.queue_changed(version);
                        }
                    }

                    // Only while playing: a paused position doesn't move, and
                    // re-sending it would keep a transport bar redrawing.
                    if state == PlaybackState::Playing {
                        let position = engine.state.position_ms();
                        if position != last_position {
                            last_position = position;
                            listener.position_changed(position);
                        }
                    }
                }
            })
            .ok();
    }

    fn db(&self) -> Result<Database, KoanError> {
        Database::open(&self.db_path).map_err(db_err)
    }

    fn send(&self, cmd: PlayerCommand) -> Result<(), KoanError> {
        self.tx.send(cmd).map_err(|e| KoanError::Player {
            message: e.to_string(),
        })
    }

    /// Resolve track IDs into playlist items, collecting the ones that still
    /// need downloading. Skips IDs that aren't in the library.
    ///
    /// One query for the rows and one config load for the batch. Doing either
    /// per track is what made adding an album — never mind an artist — take
    /// long enough to be worth a progress indicator.
    fn build_items(
        &self,
        db: &Database,
        track_ids: &[i64],
    ) -> (Vec<PlaylistItem>, Vec<(i64, QueueItemId)>) {
        let rows = queries::tracks_by_ids(&db.conn, track_ids).unwrap_or_default();
        let items = koan_core::helpers::playlist_items_for_tracks(db, &rows);
        let pending = items
            .iter()
            .filter(|item| matches!(item.load_state, LoadState::Pending))
            .filter_map(|item| item.db_id.map(|id| (id, item.id)))
            .collect();
        (items, pending)
    }

    /// Load the cursor's track, seek to `position_ms`, and stop there.
    ///
    /// Setting the position atomic alone achieves nothing — the engine has not
    /// opened the file, so the first play starts from zero. The sequence is the
    /// TUI's: play to load and seek, then immediately pause. It waits for the
    /// track to be Ready first, because a remote track is still downloading at
    /// this point, and gives up rather than waiting forever on one that fails.
    fn park_at(&self, id: QueueItemId, position_ms: u64, resume: bool) {
        if position_ms == 0 && !resume {
            return;
        }
        let state = self.state.clone();
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("koan-session-restore".into())
            .spawn(move || {
                for _ in 0..600 {
                    // The user may have started playing something in the
                    // meantime; restoring a position over that would be rude.
                    if state.playback_state() != PlaybackState::Stopped
                        || state.cursor() != Some(id)
                    {
                        return;
                    }
                    if state
                        .item_load_state(id)
                        .is_some_and(|s| matches!(s, LoadState::Ready))
                    {
                        let _ = tx.send(PlayerCommand::Play(id));
                        if position_ms > 0 {
                            let _ = tx.send(PlayerCommand::Seek(position_ms));
                        }
                        // Whether to stay parked is decided here, not by the
                        // caller: this runs on a thread that waits for the
                        // track to become ready, so a Resume sent alongside
                        // would land long before the Pause and be undone.
                        if !resume {
                            let _ = tx.send(PlayerCommand::Pause);
                        }
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                log::info!("session restore: track never became ready, leaving position at 0");
            })
            .ok();
    }

    fn start_downloads(&self, pending: Vec<(i64, QueueItemId)>) {
        if !pending.is_empty() {
            spawn_downloads(pending, self.tx.clone(), self.state.clone());
        }
    }

    /// Attach favourite state in one pass — one query per listing rather than
    /// one per row. Keyed by track ID because the favourites table stores
    /// whichever path a track happens to have, and a never-cached remote track
    /// only has a URL.
    fn decorate(&self, db: &Database, rows: Vec<queries::TrackRow>) -> Vec<Track> {
        let favs: HashSet<i64> = queries::favourite_track_ids_batch(&db.conn).unwrap_or_default();
        rows.into_iter()
            .map(|r| {
                let is_fav = favs.contains(&r.id);
                Track::from_row(r, is_fav)
            })
            .collect()
    }
}

/// Rebuild playlist items for a saved queue.
///
/// Anything still in the library is re-resolved so cache paths and downloads
/// are correct; anything that has since gone keeps the copy stored in the
/// snapshot, so a restore never silently drops tracks.
fn restore_items(
    db: &Database,
    saved: &[PersistedQueueItem],
) -> (Vec<PlaylistItem>, Vec<(i64, QueueItemId)>) {
    let ids: Vec<Option<i64>> = saved
        .iter()
        .map(|item| {
            queries::track_id_by_path(&db.conn, &item.path)
                .ok()
                .flatten()
        })
        .collect();

    let known: Vec<i64> = ids.iter().flatten().copied().collect();
    let rows = queries::tracks_by_ids(&db.conn, &known).unwrap_or_default();
    let mut resolved = koan_core::helpers::playlist_items_for_tracks(db, &rows).into_iter();

    let mut items = Vec::with_capacity(saved.len());
    let mut pending = Vec::new();
    for (saved_item, id) in saved.iter().zip(ids) {
        match id.and_then(|_| resolved.next()) {
            Some(item) => {
                if matches!(item.load_state, LoadState::Pending)
                    && let Some(db_id) = item.db_id
                {
                    pending.push((db_id, item.id));
                }
                items.push(item);
            }
            None => items.push(saved_item.to_playlist_item()),
        }
    }
    (items, pending)
}

/// Fisher-Yates over a fresh seed, so consecutive calls differ.
///
/// Deliberately not seeded from anything stable: "shuffle again" has to
/// actually produce a new order, which a process-lifetime seed wouldn't.
fn shuffle<T>(items: &mut [T]) {
    let mut seed = [0u8; 8];
    if getrandom::fill(&mut seed).is_err() {
        return; // Leave the order alone rather than pretending to shuffle.
    }
    let mut state = u64::from_le_bytes(seed) | 1;
    for i in (1..items.len()).rev() {
        // xorshift64 — plenty for shuffling a list nobody is betting on.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        items.swap(i, (state % (i as u64 + 1)) as usize);
    }
}

fn sort_rows(mut rows: Vec<queries::TrackRow>, sort: TrackSort) -> Vec<queries::TrackRow> {
    match sort {
        // Left alone deliberately. Every query already ORDERs BY album date,
        // title, disc and track — the coherent ordering, and better than
        // anything reconstructible here since TrackRow carries no release date.
        // Re-sorting on (disc, track) alone turned an artist's discography into
        // track 1 of every album, then track 2 of every album.
        TrackSort::Album => {}
        TrackSort::Title => rows.sort_by_key(|r| r.title.to_lowercase()),
        TrackSort::Artist => rows.sort_by_key(|r| r.artist_name.to_lowercase()),
        TrackSort::Duration => rows.sort_by_key(|r| r.duration_ms.unwrap_or(0)),
    }
    rows
}

/// Mirrors the GraphQL layer's behaviour: star/unstar on the remote server in a
/// detached thread so the UI never waits on the network.
fn sync_favourite_to_remote(db: &Database, path: &str, star: bool) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let Ok(Some(remote_id)) = queries::remote_id_for_path(&db.conn, Path::new(path)) else {
        return;
    };
    let Some(client) = koan_core::helpers::subsonic_client(&cfg) else {
        return;
    };
    std::thread::Builder::new()
        .name("koan-fav-sync".into())
        .spawn(move || {
            let result = if star {
                client.star(&remote_id)
            } else {
                client.unstar(&remote_id)
            };
            if let Err(e) = result {
                log::warn!("failed to sync favourite to remote: {e}");
            }
        })
        .ok();
}

fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else {
        "application/octet-stream"
    }
}

fn parse_qid(s: &str) -> Result<QueueItemId, KoanError> {
    Uuid::parse_str(s)
        .map(QueueItemId)
        .map_err(|_| KoanError::BadArgument {
            message: format!("not a queue item id: {s}"),
        })
}

fn parse_qids(ids: &[String]) -> Result<Vec<QueueItemId>, KoanError> {
    ids.iter().map(|s| parse_qid(s)).collect()
}

fn db_err(e: impl std::fmt::Display) -> KoanError {
    KoanError::Database {
        message: e.to_string(),
    }
}

fn fav_err(e: rusqlite::Error) -> KoanError {
    KoanError::Database {
        message: e.to_string(),
    }
}

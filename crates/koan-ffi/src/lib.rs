//! In-process bindings for native GUI clients.
//!
//! This is the same facade `koan-server` puts behind GraphQL, minus the wire.
//! Every method is "read `SharedPlayerState` / hit the DB / send a
//! `PlayerCommand`" — the mapping koan-core's helpers already own. A local app
//! sits on top of the audio engine, so it has no business round-tripping HTTP
//! to reach it; GraphQL stays the surface for clients that genuinely can't
//! link the core (web, iOS, jukebox remotes).
//!
//! Threading: anything that can block is `async` and runs on a worker thread,
//! so no caller ever holds a thread while koan-core reads a file or waits on a
//! socket. The few methods that stay synchronous read one atomic and nothing
//! else. See `offload` for where the work goes, and why ordering has a lane of
//! its own.
//!
//! DB connections are opened per call, matching what the GraphQL resolvers do —
//! WAL makes that cheap and it sidesteps holding a lock across a scan.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::audio::viz::VizSnapshot;
use koan_core::config::{self, Config};
use koan_core::db::connection::Database;
use koan_core::db::queries::{self, PersistedQueueItem};
use koan_core::helpers::spawn_downloads;
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{
    LoadState, PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState,
};
use koan_core::remote::client::SubsonicError;
use uuid::Uuid;

mod offload;
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

/// Something the engine changed, delivered by awaiting `next_event`.
///
/// A pull surface rather than a callback interface: a client writes a loop
/// instead of an object, and the loop's lifetime is the subscription's, so
/// there is nothing to unregister and nothing to leak across a reload.
///
/// Every variant carries an absolute value, never a delta, which is what makes
/// it safe to drop the older ones when a client falls behind — the next one
/// tells the whole truth on its own.
// One variant carries a snapshot and two carry a u64. Boxing is what clippy
// wants and is not on offer across the FFI, and the saving would be nothing:
// these are built a handful of times a second, not held in a collection.
#[allow(clippy::large_enum_variant)]
#[derive(uniffi::Enum, Debug, Clone)]
pub enum PlayerEvent {
    /// State, track, or format changed — anything a transport bar displays
    /// other than the position.
    PlaybackChanged { now_playing: NowPlaying },
    /// The queue was mutated. Carries the version so a client can skip a
    /// refetch it has already done.
    QueueChanged { version: u64 },
    /// Playback position, while playing.
    PositionChanged { position_ms: u64 },
    /// What is downloading, and how far each has got.
    ///
    /// Separate from `QueueChanged` because progress moves several times a
    /// second while the queue itself does not — announcing it as a queue change
    /// makes every client refetch the whole list for a byte counter, which is
    /// the thing the version guard exists to avoid. An empty list means nothing
    /// is in flight any more, which is also how a client learns a download
    /// finished.
    DownloadsChanged { downloads: Vec<DownloadProgress> },
    /// The set of transfers changed — one appeared, settled or was forgotten.
    ///
    /// Separate from `DownloadsChanged`, which carries byte counts and fires
    /// several times a second. This one fires when the *list* changes, which is
    /// what a client rebuilds a list on.
    DownloadStoreChanged { version: u64 },
    /// The library's rows changed — a scan, a sync, an import, an organize or
    /// a folder being forgotten. Carries a version so a client can tell one
    /// change from a repeat of the one it already handled.
    ///
    /// Says nothing about what changed. A client that holds no copy of the
    /// library only needs to know to ask again, and one that does is holding
    /// the thing this event exists to warn it about.
    LibraryChanged { version: u64 },
}

/// Reports how far a long task has got.
///
/// Scans and syncs take anywhere up to a minute, and a spinner that cannot say
/// how far through it is tells the user only that the app has not crashed.
///
/// `advanced` is called from a worker thread, often — implementations must be
/// cheap and must not block. koan throttles the calls so a fifty-thousand-file
/// scan does not cross the FFI fifty thousand times.
#[uniffi::export(with_foreign)]
pub trait ProgressReporter: Send + Sync {
    /// How many items there are, once known. Zero means unknowable.
    fn started(&self, total: u64);
    /// How many are done, and what is being worked on now.
    fn advanced(&self, done: u64, detail: String);
}

/// The player, the library, and the bridge between them.
/// Send `log` output to `~/.config/koan/koan.log`, the same file the CLI
/// writes.
///
/// Without this every `log::warn!` in koan-core is discarded when the engine is
/// hosted by a GUI, so a favourite that failed to reach the server, or a track
/// that would not decode, leaves nothing behind to look at.
fn init_logging() {
    use std::io::Write as _;
    use std::sync::Mutex;

    struct FileLogger(Mutex<Option<std::fs::File>>);

    impl log::Log for FileLogger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Info
        }

        fn log(&self, record: &log::Record) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let Ok(mut guard) = self.0.lock() else { return };
            let Some(file) = guard.as_mut() else { return };
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let _ = writeln!(
                file,
                "[{}] {}: {}",
                now,
                record.level().as_str().to_lowercase(),
                record.args()
            );
        }

        fn flush(&self) {
            if let Ok(mut guard) = self.0.lock()
                && let Some(file) = guard.as_mut()
            {
                let _ = file.flush();
            }
        }
    }

    static LOGGER: std::sync::OnceLock<FileLogger> = std::sync::OnceLock::new();

    let logger = LOGGER.get_or_init(|| {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config::config_dir().join("koan.log"))
            .ok();
        FileLogger(Mutex::new(file))
    });
    // A second engine in one process is not an error worth failing over.
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

#[derive(uniffi::Object)]
pub struct KoanEngine {
    state: Arc<SharedPlayerState>,
    tx: Sender<PlayerCommand>,
    /// The analyser's latest frame. Only the three-band summary crosses the
    /// boundary — see `viz_levels`.
    viz: Arc<VizSnapshot>,
    db_path: PathBuf,
    events: tokio::sync::broadcast::Sender<PlayerEvent>,
    /// Set while the automatic sync is running, so a UI can say so rather than
    /// appearing to do nothing for the minute it takes.
    auto_syncing: Arc<std::sync::atomic::AtomicBool>,
    /// Set while the startup or watched-folder scan is running.
    auto_scanning: Arc<std::sync::atomic::AtomicBool>,
    /// Raised to stop whichever library task is running. One flag rather than
    /// one per task, because only one runs at a time — they all contend for the
    /// same single database writer.
    cancel_library_task: Arc<std::sync::atomic::AtomicBool>,
    /// Bumped by anything that writes library rows. The watcher turns a change
    /// here into a `LibraryChanged` event, so a background scan finishing looks
    /// the same to a client as one it asked for itself.
    library_version: Arc<std::sync::atomic::AtomicU64>,
}

#[uniffi::export]
impl KoanEngine {
    /// Spawns the player thread and opens the library. One per process.
    #[uniffi::constructor]
    pub async fn new() -> Result<Arc<Self>, KoanError> {
        offload::offload(Self::build).await
    }
    // --- Transport ---------------------------------------------------------

    /// Move the cursor to `queue_item_id` and start playing it.
    pub async fn play(self: Arc<Self>, queue_item_id: String) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Play(parse_qid(&queue_item_id)?))).await
    }

    pub async fn pause(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Pause)).await
    }

    pub async fn resume(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Resume)).await
    }

    pub async fn stop(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Stop)).await
    }

    /// Space-bar behaviour: pause when playing, resume otherwise.
    pub async fn toggle_play_pause(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || match self.state.playback_state() {
            PlaybackState::Playing => self.send(PlayerCommand::Pause),
            _ => self.send(PlayerCommand::Resume),
        })
        .await
    }

    pub async fn next(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::NextTrack)).await
    }

    pub async fn previous(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::PrevTrack)).await
    }

    pub async fn seek(self: Arc<Self>, position_ms: u64) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Seek(position_ms))).await
    }

    // --- Observable state --------------------------------------------------

    /// One consistent read of everything the transport bar needs. The UI polls
    /// this; `playlist_version` tells it whether the queue also needs refetching.
    pub async fn now_playing(self: Arc<Self>) -> NowPlaying {
        offload::offload(move || self.now_playing_blocking()).await
    }
    /// Wait for the next thing to change.
    ///
    /// `None` once the engine is gone, which ends the caller's loop. A client
    /// that falls behind loses the events it missed rather than delaying the
    /// engine — every variant carries an absolute value, so the one that does
    /// arrive is still correct.
    pub async fn next_event(self: Arc<Self>) -> Option<PlayerEvent> {
        let mut rx = self.events.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => return Some(event),
                // Fell behind. The next event supersedes whatever was dropped.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    log::debug!("client missed {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Every transfer koan knows about: what it is fetching now, and what it
    /// fetched a moment ago.
    ///
    /// Running first, then whatever settled most recently. Read it on
    /// `DownloadStoreChanged` for the list and on `DownloadsChanged` for the
    /// figures — the byte counts here are a snapshot taken as this was called.
    pub fn downloads(&self) -> Vec<DownloadEntry> {
        koan_core::remote::downloads::store()
            .all()
            .iter()
            .map(DownloadEntry::from)
            .collect()
    }

    /// How many transfers are actually moving. Cheap enough for a sidebar to
    /// read on every redraw.
    pub fn active_download_count(&self) -> u32 {
        koan_core::remote::downloads::store().active() as u32
    }

    /// Forget the transfers that have already settled. Running ones are left
    /// alone — stopping one is a different verb.
    pub fn clear_settled_downloads(&self) {
        koan_core::remote::downloads::store().clear_settled();
    }

    /// Cheap enough to poll every frame — use it to decide whether to call
    /// `queue()`, which allocates the whole list.
    pub fn playlist_version(&self) -> u64 {
        self.state.playlist_version()
    }

    /// What is coming out of the speakers right now, as three band energies.
    ///
    /// Sync, like `playlist_version`: an uncontended read lock and a reduce
    /// over 48 floats, with nothing to allocate and nothing that can block. A
    /// caller polling this at 30 Hz would spend more on the async hop than on
    /// the read.
    pub fn viz_levels(&self) -> VizLevels {
        self.viz.levels().into()
    }

    pub async fn queue(self: Arc<Self>) -> Vec<QueueItem> {
        offload::offload(move || {
            let entries = self.state.derive_visible_queue().entries;
            // One query for the whole queue. A client draws one sleeve per
            // album, and without an ID to group by it asks for artwork per
            // track — the same image fetched once for every track on the
            // record. A queue with no database behind it simply has no album
            // IDs; the art falls back to the per-track lookup as before.
            let track_ids: Vec<i64> = entries.iter().filter_map(|e| e.db_id).collect();
            // Two questions of the same rows, asked once each for the whole
            // queue rather than once per row.
            let db = self.db().ok();
            let album_ids = db
                .as_ref()
                .and_then(|db| queries::batch::album_ids_for_tracks(&db.conn, &track_ids).ok())
                .unwrap_or_default();
            let sources = db
                .as_ref()
                .and_then(|db| queries::batch::sources_for_tracks(&db.conn, &track_ids).ok())
                .unwrap_or_default();

            entries
                .iter()
                .map(|e| QueueItem::from_entry(e, &album_ids, &sources))
                .collect()
        })
        .await
    }

    // --- Queue mutation ----------------------------------------------------

    /// Append tracks. Starts playback if the player was stopped, and kicks off
    /// downloads for anything remote. Returns the new queue item IDs.
    pub async fn add_to_queue(
        self: Arc<Self>,
        track_ids: Vec<i64>,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
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
        })
        .await
    }

    /// Clear the queue and play `track_ids` from the top.
    /// Replace the queue, starting at `start_at` (default: the first track).
    ///
    /// The index is part of the command rather than a follow-up `play` because
    /// two commands means the first track audibly starts before the jump lands:
    /// clicking track nine of an album flashed track one as playing first.
    /// An index past the end starts at the beginning.
    pub async fn replace_queue(
        self: Arc<Self>,
        track_ids: Vec<i64>,
        start_at: Option<u32>,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
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
        })
        .await
    }

    /// Insert after an existing item — what a drop between two rows means.
    pub async fn insert_after(
        self: Arc<Self>,
        track_ids: Vec<i64>,
        after_queue_item_id: String,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
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
        })
        .await
    }

    /// Removed as one undo step, however many IDs are passed.
    pub async fn remove_from_queue(
        self: Arc<Self>,
        queue_item_ids: Vec<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let ids = parse_qids(&queue_item_ids)?;
            self.send(PlayerCommand::RemoveFromPlaylistBatch(ids))
        })
        .await
    }

    /// Reorder. `after` puts the items below the target rather than above.
    pub async fn move_in_queue(
        self: Arc<Self>,
        queue_item_ids: Vec<String>,
        target_queue_item_id: String,
        after: bool,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let ids = parse_qids(&queue_item_ids)?;
            let target = parse_qid(&target_queue_item_id)?;
            self.send(PlayerCommand::MoveItemsInPlaylist { ids, target, after })
        })
        .await
    }

    pub async fn clear_queue(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::ClearPlaylist)).await
    }

    pub async fn undo(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Undo)).await
    }

    pub async fn redo(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Redo)).await
    }

    // --- Library -----------------------------------------------------------

    /// The library's artists, narrowed by `search`.
    ///
    /// Whole, not paged. This is an in-process call, and a library's artists
    /// are a bounded set — a few thousand records marshalled once beats a
    /// client that has to know how far it has scrolled.
    pub async fn artists(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Artist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::list_artists(
                &db.conn,
                &queries::ArtistQuery {
                    search: trimmed(&search),
                    ..Default::default()
                },
            )
            .map_err(db_err)?;
            Ok(rows.into_iter().map(Artist::from).collect())
        })
        .await
    }

    pub async fn artist(self: Arc<Self>, artist_id: i64) -> Result<Option<Artist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::get_artist(&db.conn, artist_id)
                .map_err(db_err)?
                .map(Artist::from))
        })
        .await
    }

    /// The library's albums, narrowed by `search` and ordered by `sort`.
    ///
    /// Both run in SQL. A client that narrows or sorts what it has already been
    /// handed pays to read and marshal every album in the library on each
    /// keystroke, and has to reimplement in its own language an answer the
    /// database already knows.
    ///
    /// Whole, not paged, for the reason [`Self::artists`] gives.
    ///
    /// `seed` fixes the shuffle under [`AlbumSort::Random`] and is ignored by
    /// every other sort, so that narrowing a shuffled listing does not deal it
    /// again. A new seed is a new shuffle, which is what a reshuffle asks for.
    pub async fn albums(
        self: Arc<Self>,
        artist_id: Option<i64>,
        sort: AlbumSort,
        seed: i64,
        search: Option<String>,
    ) -> Result<Vec<Album>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    artist_id,
                    search: trimmed(&search),
                    order: album_order(sort, seed),
                    ..Default::default()
                },
            )
            .map_err(db_err)?;
            Ok(rows.into_iter().map(Album::from).collect())
        })
        .await
    }

    pub async fn album(self: Arc<Self>, album_id: i64) -> Result<Option<Album>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::get_album(&db.conn, album_id)
                .map_err(db_err)?
                .map(Album::from))
        })
        .await
    }

    /// Tracks for an album or artist, or a page of the whole library when
    /// neither is given.
    pub async fn tracks(
        self: Arc<Self>,
        album_id: Option<i64>,
        artist_id: Option<i64>,
        sort: TrackSort,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Track>, KoanError> {
        offload::offload(move || self.tracks_blocking(album_id, artist_id, sort, limit, offset))
            .await
    }

    pub async fn track(self: Arc<Self>, track_id: i64) -> Result<Option<Track>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let Some(row) = queries::get_track_row(&db.conn, track_id).map_err(db_err)? else {
                return Ok(None);
            };
            Ok(self.decorate(&db, vec![row]).into_iter().next())
        })
        .await
    }

    /// FTS5 search across title, artist, album, genre.
    pub async fn search(
        self: Arc<Self>,
        query: String,
        limit: u32,
    ) -> Result<Vec<Track>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::search_tracks_paged(&db.conn, &query, limit, 0).map_err(db_err)?;
            Ok(self.decorate(&db, rows))
        })
        .await
    }

    /// Nucleo fuzzy match — what the command palette wants. Ranked, best first.
    pub async fn fuzzy_search(
        self: Arc<Self>,
        query: String,
        kind: SearchKind,
        limit: u32,
    ) -> Result<Vec<FuzzyMatch>, KoanError> {
        offload::offload(move || {
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

            let texts: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
            Ok(fuzzy_rank(&texts, &query, limit)
                .into_iter()
                .map(|i| FuzzyMatch {
                    id: items[i].0,
                    name: items[i].1.clone(),
                    kind,
                })
                .collect())
        })
        .await
    }

    /// Fuzzy-matched albums, as rows.
    ///
    /// Rows rather than ids: the match already read them to build its corpus,
    /// and a caller handed ids can only resolve them against a catalogue of its
    /// own — which is the copy this exists to make unnecessary.
    pub async fn fuzzy_albums(
        self: Arc<Self>,
        query: String,
        limit: u32,
    ) -> Result<Vec<Album>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::all_albums(&db.conn).map_err(db_err)?;
            let texts: Vec<String> = rows
                .iter()
                .map(|a| format!("{} — {}", a.artist_name, a.title))
                .collect();
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            Ok(fuzzy_rank(&texts, &query, limit)
                .into_iter()
                .map(|i| Album::from(rows[i].clone()))
                .collect())
        })
        .await
    }

    /// Fuzzy-matched artists, as rows. See [`Self::fuzzy_albums`].
    pub async fn fuzzy_artists(
        self: Arc<Self>,
        query: String,
        limit: u32,
    ) -> Result<Vec<Artist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::all_artists(&db.conn).map_err(db_err)?;
            let texts: Vec<&str> = rows.iter().map(|a| a.name.as_str()).collect();
            Ok(fuzzy_rank(&texts, &query, limit)
                .into_iter()
                .map(|i| Artist::from(rows[i].clone()))
                .collect())
        })
        .await
    }

    pub async fn random_tracks(
        self: Arc<Self>,
        count: u32,
        artist_id: Option<i64>,
    ) -> Result<Vec<Track>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::random_tracks(&db.conn, count, artist_id).map_err(db_err)?;
            Ok(self.decorate(&db, rows))
        })
        .await
    }

    pub async fn similar_artists(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<Vec<SimilarArtist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows =
                queries::get_similar_artists_detailed(&db.conn, artist_id).map_err(db_err)?;
            Ok(rows
                .into_iter()
                .map(|e| SimilarArtist {
                    artist_id: e.artist.id,
                    name: e.artist.name,
                    score: e.score,
                    source: e.source,
                })
                .collect())
        })
        .await
    }

    pub async fn library_stats(self: Arc<Self>) -> Result<Stats, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::library_stats(&db.conn).map_err(db_err)?.into())
        })
        .await
    }

    /// Raw image bytes — no base64 round trip, unlike the GraphQL surface,
    /// which has to encode because JSON can't carry binary.
    ///
    /// Embedded tags first, then the remote server. A library synced from
    /// Navidrome has no local files to read art out of, so without the remote
    /// fallback every album is blank. `size` requests a thumbnail; the grid
    /// wants one, the now-playing pane doesn't. Hits the network on the remote
    /// path.
    pub async fn cover_art(
        self: Arc<Self>,
        track_id: i64,
        size: Option<u32>,
    ) -> Result<Option<CoverArt>, KoanError> {
        offload::offload(move || {
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
            let cfg = Config::cached();
            // No server configured: this record simply has no art.
            if !cfg.remote.enabled {
                return Ok(None);
            }
            // Configured but unusable — signed out, or the password cannot be
            // read. Reported rather than shrugged off: answering "no art" for
            // every record makes a signed-out client look like a library that
            // has no covers, which is a long way from where the problem is.
            let Some(client) = koan_core::helpers::subsonic_client(&cfg) else {
                return Err(KoanError::Remote {
                    message: koan_core::helpers::remote_unavailable(&cfg),
                });
            };
            match client.get_cover_art(&remote_id, size) {
                Ok(data) if !data.is_empty() => {
                    let mime = sniff_mime(&data).to_string();
                    Ok(Some(CoverArt { data, mime }))
                }
                // The server answered and it has nothing. Normal, and worth
                // remembering: this record has no art and never will.
                Ok(_) | Err(SubsonicError::Api { .. }) | Err(SubsonicError::BadResponse) => {
                    Ok(None)
                }
                // A timeout or a dropped connection says nothing about whether
                // art exists. Reported rather than swallowed, so the caller can
                // ask again instead of recording "this album has none" for the
                // rest of the session — and so it appears in the log at all.
                Err(e) => {
                    log::warn!("cover art for track {track_id} failed: {e}");
                    Err(KoanError::Remote {
                        message: e.to_string(),
                    })
                }
            }
        })
        .await
    }

    /// Why the configured server cannot be used, if it cannot.
    ///
    /// `None` means there is nothing to say: either no server is configured, or
    /// the one that is works. A client should not have to watch playback fail
    /// and artwork come back empty to work out that it is signed out — the
    /// engine already knows, and every front end asks the same question.
    pub async fn remote_problem(self: Arc<Self>) -> Option<String> {
        offload::offload(move || {
            let cfg = Config::cached();
            if !cfg.remote.enabled || cfg.remote.url.is_empty() {
                return None;
            }
            koan_core::helpers::subsonic_auth(&cfg)
                .is_none()
                .then(|| koan_core::helpers::remote_unavailable(&cfg))
        })
        .await
    }

    /// Cached lyrics only — this never hits the network, so it is safe to call
    /// from a view body's task without stalling on LRCLIB.
    pub async fn lyrics(self: Arc<Self>, track_id: i64) -> Result<Option<Lyrics>, KoanError> {
        offload::offload(move || {
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
        })
        .await
    }

    /// Cache, then LRCLIB. Hits the network on a miss; `lyrics()` answers from
    /// the cache alone and returns without one.
    pub async fn fetch_lyrics(self: Arc<Self>, track_id: i64) -> Result<Option<Lyrics>, KoanError> {
        offload::offload(move || {
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
        })
        .await
    }

    // --- Play history ------------------------------------------------------

    /// Every play, most recent first, narrowed by `search`.
    ///
    /// A list of events, not of tracks: a track played three times is three
    /// entries. Entries whose track has left the library are already gone.
    ///
    /// Whole, not paged, for the reason [`Self::artists`] gives.
    pub async fn play_history(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<PlayHistoryEntry>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::play_history_with_tracks(&db.conn, trimmed(&search), None, 0)
                .map_err(db_err)?;
            let (plays, tracks): (Vec<_>, Vec<_>) = rows
                .into_iter()
                .map(|r| ((r.id, r.played_at, r.listened_ms, r.source), r.track))
                .unzip();
            Ok(self
                .decorate(&db, tracks)
                .into_iter()
                .zip(plays)
                .map(
                    |(track, (id, played_at, listened_ms, source))| PlayHistoryEntry {
                        id,
                        track,
                        played_at,
                        listened_ms,
                        source,
                    },
                )
                .collect())
        })
        .await
    }

    /// How many times a track has been played.
    pub async fn play_count(self: Arc<Self>, track_id: i64) -> Result<i64, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::play_count(&db.conn, track_id).map_err(db_err)
        })
        .await
    }

    /// Forget specific plays. Returns how many entries were removed.
    pub async fn delete_plays(self: Arc<Self>, ids: Vec<i64>) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let removed = queries::delete_plays(&db.conn, &ids).map_err(db_err)?;
            Ok(removed as u32)
        })
        .await
    }

    /// Forget every play. Returns how many entries were removed.
    pub async fn clear_play_history(self: Arc<Self>) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let removed = queries::clear_play_history(&db.conn).map_err(db_err)?;
            Ok(removed as u32)
        })
        .await
    }

    // --- Favourites --------------------------------------------------------

    /// Favourited tracks, narrowed by `search`.
    pub async fn favourites(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Track>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::favourite_tracks(&db.conn, trimmed(&search)).map_err(db_err)?;
            Ok(self.decorate(&db, rows))
        })
        .await
    }

    /// Favourited records, as rows, narrowed by `search`.
    pub async fn favourite_albums(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Album>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    search: trimmed(&search),
                    favourites_only: true,
                    ..Default::default()
                },
            )
            .map_err(db_err)?;
            Ok(rows.into_iter().map(Album::from).collect())
        })
        .await
    }

    /// Favourited artists, as rows, narrowed by `search`.
    pub async fn favourite_artists(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Artist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::list_artists(
                &db.conn,
                &queries::ArtistQuery {
                    search: trimmed(&search),
                    favourites_only: true,
                    ..Default::default()
                },
            )
            .map_err(db_err)?;
            Ok(rows.into_iter().map(Artist::from).collect())
        })
        .await
    }

    /// Returns the new state. Syncs to the remote server in the background when
    /// one is configured.
    pub async fn toggle_favourite(self: Arc<Self>, track_id: i64) -> Result<bool, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let path = queries::track_favourite_key(&db.conn, track_id)
                .map_err(db_err)?
                .ok_or_else(|| KoanError::NotFound {
                    message: format!("track {track_id}"),
                })?;

            let now_favourite =
                queries::toggle_favourite(&db.conn, Path::new(&path)).map_err(fav_err)?;
            koan_core::helpers::sync_favourite_to_remote(&db, Path::new(&path), now_favourite);
            Ok(now_favourite)
        })
        .await
    }

    /// Every favourited track id, for the UI to read row state from one place
    /// rather than from a copy baked into each row when it was fetched.
    pub async fn favourite_track_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::favourite_track_ids_batch(&db.conn)
                .map_err(db_err)?
                .into_iter()
                .collect())
        })
        .await
    }

    pub async fn favourite_album_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::favourite_album_id_set(&db.conn)
                .map_err(fav_err)?
                .into_iter()
                .collect())
        })
        .await
    }

    pub async fn favourite_artist_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::favourite_artist_id_set(&db.conn)
                .map_err(fav_err)?
                .into_iter()
                .collect())
        })
        .await
    }

    /// Toggle an album favourite. Returns the new state.
    pub async fn toggle_favourite_album(self: Arc<Self>, album_id: i64) -> Result<bool, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let (artist, title) = queries::album_favourite_key(&db.conn, album_id)
                .map_err(db_err)?
                .ok_or_else(|| KoanError::NotFound {
                    message: format!("album {album_id}"),
                })?;
            let now =
                queries::toggle_favourite_album(&db.conn, &artist, &title).map_err(fav_err)?;
            koan_core::helpers::sync_collection_favourite_to_remote(
                &db,
                koan_core::helpers::FavouriteKind::Album,
                album_id,
                now,
            );
            Ok(now)
        })
        .await
    }

    /// Toggle an artist favourite. Returns the new state.
    pub async fn toggle_favourite_artist(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<bool, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let name = queries::artist_favourite_key(&db.conn, artist_id)
                .map_err(db_err)?
                .ok_or_else(|| KoanError::NotFound {
                    message: format!("artist {artist_id}"),
                })?;
            let now = queries::toggle_favourite_artist(&db.conn, &name).map_err(fav_err)?;
            koan_core::helpers::sync_collection_favourite_to_remote(
                &db,
                koan_core::helpers::FavouriteKind::Artist,
                artist_id,
                now,
            );
            Ok(now)
        })
        .await
    }

    // --- Playlists ---------------------------------------------------------
    //
    // A playlist is a named, ordered list of library tracks — the same object
    // Navidrome holds, so the two can be reconciled. Every edit writes locally
    // and then pushes to the server in the background; nothing waits on the
    // network, and a push that never got out is settled by the next sync.

    pub async fn playlists(self: Arc<Self>) -> Result<Vec<Playlist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::list_playlists(&db.conn)
                .map_err(db_err)?
                .into_iter()
                .map(Playlist::from)
                .collect())
        })
        .await
    }

    pub async fn playlist(
        self: Arc<Self>,
        playlist_id: i64,
    ) -> Result<Option<Playlist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::get_playlist(&db.conn, playlist_id)
                .map_err(db_err)?
                .map(Playlist::from))
        })
        .await
    }

    /// The playlist's tracks, in playlist order. Duplicates are kept — the same
    /// song twice in a row is a thing people do on purpose.
    pub async fn playlist_tracks(
        self: Arc<Self>,
        playlist_id: i64,
    ) -> Result<Vec<PlaylistEntry>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let entries = queries::playlist_entries(&db.conn, playlist_id).map_err(db_err)?;
            let ids: Vec<i64> = entries.iter().map(|e| e.id).collect();
            let tracks = self.decorate(&db, entries.into_iter().map(|e| e.track).collect());
            Ok(ids
                .into_iter()
                .zip(tracks)
                .map(|(id, track)| PlaylistEntry { id, track })
                .collect())
        })
        .await
    }

    /// The playlist the queue is still exactly, if it is one.
    ///
    /// While this answers, the queue follows that playlist: an edit there lands
    /// here too. It stops answering the moment the queue is rearranged, added
    /// to, or extended by radio — which is also when the following stops.
    pub async fn queue_lock(self: Arc<Self>) -> Result<Option<QueueLock>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(match koan_core::playlists::queue_lock(&db, &self.state) {
                Some(koan_core::playlists::QueueLock::Playlist(id)) => {
                    queries::get_playlist(&db.conn, id)
                        .map_err(db_err)?
                        .map(|p| QueueLock::Playlist {
                            playlist: Playlist::from(p),
                        })
                }
                Some(koan_core::playlists::QueueLock::Album(id)) => {
                    queries::get_album(&db.conn, id)
                        .map_err(db_err)?
                        .map(|a| QueueLock::Album {
                            album: Album::from(a),
                        })
                }
                None => None,
            })
        })
        .await
    }

    /// Up to four tracks whose covers make the playlist's tile — one per album,
    /// in playlist order.
    pub async fn playlist_cover_track_ids(
        self: Arc<Self>,
        playlist_id: i64,
    ) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::playlist_cover_track_ids(&db.conn, playlist_id).map_err(db_err)
        })
        .await
    }

    pub async fn create_playlist(
        self: Arc<Self>,
        name: String,
        track_ids: Vec<i64>,
    ) -> Result<Playlist, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let id = queries::create_playlist(&db.conn, &name, None).map_err(db_err)?;
            if !track_ids.is_empty() {
                queries::add_tracks(&db.conn, id, &track_ids).map_err(db_err)?;
            }
            koan_core::playlists::push_to_remote(id);
            queries::get_playlist(&db.conn, id)
                .map_err(db_err)?
                .map(Playlist::from)
                .ok_or_else(|| KoanError::NotFound {
                    message: format!("playlist {id}"),
                })
        })
        .await
    }

    /// The current queue, kept. Items with no library row behind them — a file
    /// played before it was indexed — cannot come across, because a playlist
    /// points at library rows.
    pub async fn save_queue_as_playlist(
        self: Arc<Self>,
        name: String,
    ) -> Result<Playlist, KoanError> {
        let track_ids: Vec<i64> = self
            .state
            .snapshot_playlist()
            .0
            .iter()
            .filter_map(|item| item.db_id)
            .collect();
        self.create_playlist(name, track_ids).await
    }

    pub async fn rename_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        name: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::rename_playlist(&db.conn, playlist_id, &name).map_err(db_err)?;
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(())
        })
        .await
    }

    pub async fn delete_playlist(self: Arc<Self>, playlist_id: i64) -> Result<bool, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            // Read the server id before the row goes: the delete has to reach
            // the server too, or the next sync brings the playlist back.
            let remote_id = queries::get_playlist(&db.conn, playlist_id)
                .ok()
                .flatten()
                .and_then(|p| p.remote_id);
            let deleted = queries::delete_playlist(&db.conn, playlist_id).map_err(db_err)?;
            if deleted && let Some(remote_id) = remote_id {
                koan_core::playlists::delete_on_remote(remote_id);
            }
            Ok(deleted)
        })
        .await
    }

    /// Append tracks. Returns how many landed.
    pub async fn add_to_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        track_ids: Vec<i64>,
    ) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let locked = self.locked_to(&db, playlist_id);
            let added = queries::add_tracks(&db.conn, playlist_id, &track_ids).map_err(db_err)?;
            self.follow_playlist(&db, playlist_id, locked);
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(added.len() as u32)
        })
        .await
    }

    /// Add tracks at a position rather than at the end — what dropping between
    /// two rows means.
    ///
    /// Both halves happen here rather than as an add followed by a reorder from
    /// the caller: the caller would be reordering against the list as it was
    /// before its own insert landed, and racing the reload that tells it
    /// otherwise.
    pub async fn insert_into_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        track_ids: Vec<i64>,
        at: u32,
    ) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let locked = self.locked_to(&db, playlist_id);
            let added = queries::add_tracks(&db.conn, playlist_id, &track_ids).map_err(db_err)?;
            if !added.is_empty() {
                let mut order: Vec<i64> = queries::playlist_entries(&db.conn, playlist_id)
                    .map_err(db_err)?
                    .into_iter()
                    .map(|e| e.id)
                    .filter(|id| !added.contains(id))
                    .collect();
                let at = (at as usize).min(order.len());
                order.splice(at..at, added.iter().copied());
                queries::reorder_entries(&db.conn, playlist_id, &order).map_err(db_err)?;
            }
            self.follow_playlist(&db, playlist_id, locked);
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(added.len() as u32)
        })
        .await
    }

    /// Put the entries in this order. Ids survive, so anything holding one —
    /// a queue item, say — still knows which row it means.
    pub async fn reorder_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        entry_ids: Vec<i64>,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let locked = self.locked_to(&db, playlist_id);
            queries::reorder_entries(&db.conn, playlist_id, &entry_ids).map_err(db_err)?;
            self.follow_playlist(&db, playlist_id, locked);
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(())
        })
        .await
    }

    /// Take entries out. Returns how many went.
    pub async fn remove_from_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        entry_ids: Vec<i64>,
    ) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let locked = self.locked_to(&db, playlist_id);
            let removed =
                queries::remove_entries(&db.conn, playlist_id, &entry_ids).map_err(db_err)?;
            self.follow_playlist(&db, playlist_id, locked);
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(removed as u32)
        })
        .await
    }

    /// Shuffle the playlist itself, in place. Distinct from shuffling it into
    /// the queue, which leaves the playlist alone.
    pub async fn shuffle_playlist(self: Arc<Self>, playlist_id: i64) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let mut entries = queries::playlist_entries(&db.conn, playlist_id).map_err(db_err)?;
            koan_core::helpers::shuffle(&mut entries);
            let order: Vec<i64> = entries.iter().map(|e| e.id).collect();
            let locked = self.locked_to(&db, playlist_id);
            queries::reorder_entries(&db.conn, playlist_id, &order).map_err(db_err)?;
            self.follow_playlist(&db, playlist_id, locked);
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(())
        })
        .await
    }

    /// Where the playlists sit in the sidebar, in the order given. Local only —
    /// no server has anywhere to put it.
    pub async fn reorder_playlists(self: Arc<Self>, ids: Vec<i64>) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::reorder_playlists(&db.conn, &ids).map_err(db_err)
        })
        .await
    }

    /// Remember whether this playlist is looked at grouped by album. `None`
    /// follows the app default, which for a playlist is ungrouped: a playlist
    /// is a sequence someone chose, not a shelf of records.
    pub async fn set_playlist_grouped(
        self: Arc<Self>,
        playlist_id: i64,
        grouped: Option<bool>,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::set_playlist_grouped(&db.conn, playlist_id, grouped).map_err(db_err)
        })
        .await
    }

    /// Replace the queue with the playlist and start playing at `start_at`.
    ///
    /// `shuffled` orders the queue, not the playlist — the playlist on disk is
    /// untouched.
    pub async fn play_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        start_at: Option<u32>,
        shuffled: bool,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
            let db = self.db()?;
            let mut entries = queries::playlist_entries(&db.conn, playlist_id).map_err(db_err)?;
            if shuffled {
                koan_core::helpers::shuffle(&mut entries);
            }
            let track_ids: Vec<i64> = entries.iter().map(|e| e.track.id).collect();

            let (mut items, pending) = self.build_items(&db, &track_ids);
            // Each queue item remembers the row it came from. The queue is an
            // ephemeral view onto the playlist and may be shuffled, cut about
            // or added to; this is what still says which row is playing, and
            // which of two copies of a song it is.
            //
            // Zipped against the entries whose track actually resolved, not
            // against all of them: a playlist naming a track the library has
            // since lost yields fewer items than entries, and zipping the two
            // directly would hand every entry after the gap the wrong id.
            let resolved: std::collections::HashSet<i64> =
                items.iter().filter_map(|i| i.db_id).collect();
            let kept = entries.iter().filter(|e| resolved.contains(&e.track.id));
            for (item, entry) in items.iter_mut().zip(kept) {
                item.playlist_entry_id = Some(entry.id);
            }
            if items.is_empty() {
                self.send(PlayerCommand::ClearPlaylist)?;
                return Ok(Vec::new());
            }

            let ids: Vec<String> = items.iter().map(|i| i.id.0.to_string()).collect();
            self.send(PlayerCommand::ReplacePlaylist {
                items,
                start: if shuffled {
                    0
                } else {
                    start_at.unwrap_or(0) as usize
                },
            })?;
            self.start_downloads(pending);

            Ok(ids)
        })
        .await
    }

    /// Write the playlist out as an extended M3U8.
    ///
    /// Only tracks with a file on this machine can go in it: a playlist file is
    /// a list of paths, and writing stream URLs instead would put the
    /// credentials that authorise them into a file people mail to each other.
    pub async fn export_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        dest_path: String,
    ) -> Result<PlaylistExport, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let summary =
                koan_core::playlists::export_m3u8(&db, playlist_id, Path::new(&dest_path))
                    .map_err(|e| KoanError::BadArgument {
                        message: e.to_string(),
                    })?;
            Ok(PlaylistExport {
                written: summary.written as u32,
                skipped: summary.skipped as u32,
            })
        })
        .await
    }

    // --- Session persistence -----------------------------------------------

    /// Write the queue and position so the next launch can pick them up.
    /// Call it on quit; it is cheap enough to call on a timer too.
    pub async fn save_session(self: Arc<Self>) -> Result<(), KoanError> {
        offload::offload(move || {
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
        })
        .await
    }

    /// Persist where you are, without rewriting the queue.
    ///
    /// Cheap enough to call every second, which is what makes a crash cost a
    /// second of playback rather than the whole session. `save_session` still
    /// runs when the queue changes and on quit.
    pub async fn save_position(self: Arc<Self>) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let (items, cursor) = self.state.snapshot_playlist();
            let cursor_path = cursor.and_then(|cid| {
                items
                    .iter()
                    .find(|i| i.id == cid)
                    .map(|i| i.path.to_string_lossy().into_owned())
            });
            queries::save_playback_position(
                &db.conn,
                cursor_path.as_deref(),
                self.state.position_ms(),
                self.state.playback_state() == PlaybackState::Playing,
                self.state.radio_mode(),
            )
            .map_err(fav_err)
        })
        .await
    }

    /// Restore the queue saved by `save_session`, cursor and position included.
    ///
    /// Resumes only if playback was running when the session was saved: closing
    /// a player mid-track and having it pick up where it left off is the point,
    /// while a player that was paused should stay paused rather than start
    /// making noise at whoever opened it.
    ///
    /// Returns the number of items restored.
    pub async fn restore_session(self: Arc<Self>) -> Result<u32, KoanError> {
        offload::sequenced(move || {
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
        })
        .await
    }

    // --- Output device -----------------------------------------------------

    pub async fn devices(self: Arc<Self>) -> Result<Vec<Device>, KoanError> {
        offload::offload(move || {
            let devices =
                koan_core::audio::list_output_devices().map_err(|e| KoanError::Audio {
                    message: e.to_string(),
                })?;
            Ok(devices
                .into_iter()
                .map(|d| Device {
                    name: d.name,
                    sample_rates: d.sample_rates,
                })
                .collect())
        })
        .await
    }

    pub async fn set_device(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::SetOutputDevice(name))).await
    }

    pub async fn clear_device(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::ClearOutputDevice)).await
    }

    /// The device name persisted in config, or `None` for the system default.
    /// Read from config rather than the player so it survives a restart.
    pub async fn current_device(self: Arc<Self>) -> Option<String> {
        offload::offload(move || Config::cached().playback.output_device.clone()).await
    }

    // --- Radio -------------------------------------------------------------

    /// Radio keeps the queue topped up with tracks chosen by similarity to
    /// what you've been listening to. The picking loop is spawned with the
    /// engine and watches this flag.
    pub fn set_radio(&self, enabled: bool) {
        self.state.set_radio_mode(enabled);
    }

    // --- Library maintenance ----------------------------------------------

    // --- Settings ----------------------------------------------------------

    /// The whole configuration, as the settings window shows it.
    pub async fn settings(self: Arc<Self>) -> Settings {
        offload::offload(move || {
            let cfg = Config::load().unwrap_or_default();
            let cache_dir = cfg.cache_dir();
            let cache_bytes = koan_core::helpers::cache_size_bytes(&cfg);

            let db = self.db().ok();
            Settings {
                library_folders: cfg
                    .library
                    .folders
                    .iter()
                    .map(|p| LibraryFolder {
                        path: p.to_string_lossy().into_owned(),
                        tracks: db
                            .as_ref()
                            .map(|db| koan_core::helpers::tracks_under(db, p))
                            .unwrap_or(0),
                    })
                    .collect(),

                remote_enabled: cfg.remote.enabled,
                remote_url: cfg.remote.url.clone(),
                remote_username: cfg.remote.username.clone(),
                remote_signed_in: koan_core::helpers::get_remote_password(&cfg).is_some(),
                remote_tracks: db
                    .as_ref()
                    .map(koan_core::helpers::tracks_from_server)
                    .unwrap_or(0),
                download_workers: cfg.remote.download_workers as u32,
                cache_limit: cfg.remote.cache_limit.clone().unwrap_or_default(),
                cache_dir: cache_dir.to_string_lossy().into_owned(),
                cache_bytes,
                auto_sync: cfg.remote.auto_sync,
                auto_sync_interval_mins: cfg.remote.auto_sync_interval_mins,

                replaygain: match cfg.playback.replaygain {
                    config::ReplayGainMode::Off => "off".into(),
                    config::ReplayGainMode::Track => "track".into(),
                    config::ReplayGainMode::Album => "album".into(),
                },
                pre_amp_db: cfg.playback.pre_amp_db,

                radio_lookahead: cfg.radio.lookahead as u32,
                radio_batch_size: cfg.radio.batch_size as u32,
                radio_discovery_weight: cfg.radio.discovery_weight,
            }
        })
        .await
    }

    /// Write the settings back.
    ///
    /// Each setting lands in the file that owns it — `Config::persist` routes
    /// folders and the server account to `config.local.toml` and taste like
    /// ReplayGain to `config.toml`. The password is not here; it goes through
    /// `sign_in_remote`.
    pub async fn update_settings(self: Arc<Self>, s: Settings) -> Result<(), KoanError> {
        offload::offload(move || {
            Config::persist(|cfg| {
                cfg.library.folders = s
                    .library_folders
                    .iter()
                    .map(|f| PathBuf::from(&f.path))
                    .collect();

                cfg.remote.enabled = s.remote_enabled;
                cfg.remote.url = s.remote_url.clone();
                cfg.remote.username = s.remote_username.clone();
                cfg.remote.download_workers = s.download_workers.max(1) as usize;
                cfg.remote.cache_limit = (!s.cache_limit.is_empty()).then(|| s.cache_limit.clone());
                cfg.remote.auto_sync = s.auto_sync;
                cfg.remote.auto_sync_interval_mins = s.auto_sync_interval_mins;

                cfg.playback.replaygain = match s.replaygain.as_str() {
                    "track" => config::ReplayGainMode::Track,
                    "album" => config::ReplayGainMode::Album,
                    _ => config::ReplayGainMode::Off,
                };
                cfg.playback.pre_amp_db = s.pre_amp_db;

                cfg.radio.lookahead = s.radio_lookahead as usize;
                cfg.radio.batch_size = (s.radio_batch_size.max(1)) as usize;
                cfg.radio.discovery_weight = s.radio_discovery_weight.clamp(0.0, 1.0);
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
        })
        .await
    }

    /// Whether the automatic library sync is running right now.
    pub fn is_auto_syncing(&self) -> bool {
        self.auto_syncing.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Ask the running library task to stop.
    ///
    /// It stops between transactions and keeps what it had already committed —
    /// a cancelled scan is a shorter scan, not an undone one.
    pub fn cancel_library_task(&self) {
        self.cancel_library_task
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the startup or watched-folder scan is running right now.
    pub fn is_auto_scanning(&self) -> bool {
        self.auto_scanning
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Sign in to a Subsonic/Navidrome server.
    ///
    /// Checked against the server before anything is written, and the password
    /// goes to the platform credential store rather than to a file.
    pub async fn sign_in_remote(
        self: Arc<Self>,
        url: String,
        username: String,
        password: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            koan_core::helpers::set_remote_credentials(&url, &username, &password).map_err(|e| {
                KoanError::BadArgument {
                    message: e.to_string(),
                }
            })
        })
        .await
    }

    /// Forget the server. Leaves the synced library alone — those tracks are
    /// still real, they just cannot be fetched until you sign in again.
    pub async fn sign_out_remote(self: Arc<Self>) -> Result<(), KoanError> {
        offload::offload(move || {
            Config::persist(|cfg| {
                cfg.remote.enabled = false;
                cfg.remote.password = String::new();
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
        })
        .await
    }

    /// Forget every track that came from a folder.
    ///
    /// A track the server also has keeps its row and loses only its local path.
    /// Albums and artists left holding nothing go too.
    pub async fn forget_folder(self: Arc<Self>, path: String) -> Result<u64, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let removed =
                koan_core::helpers::forget_folder(&db, Path::new(&path)).map_err(db_err)?;
            self.bump_library();
            Ok(removed)
        })
        .await
    }

    /// Forget everything that only existed on the server.
    ///
    /// A track held locally as well keeps its row and loses its remote id.
    pub async fn forget_remote(self: Arc<Self>) -> Result<u64, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let removed = koan_core::helpers::forget_remote(&db).map_err(db_err)?;
            self.bump_library();
            Ok(removed)
        })
        .await
    }

    /// Drop the library index so the next scan rebuilds it.
    ///
    /// Favourites survive — they key on the file path. Lyrics, play history and
    /// acoustic embeddings do not; they key on row ids that are about to stop
    /// existing.
    pub async fn rebuild_index(self: Arc<Self>) -> Result<RebuildSummary, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let summary = koan_core::helpers::rebuild_index(&db).map_err(db_err)?;
            self.bump_library();
            Ok(RebuildSummary {
                tracks: summary.tracks,
                albums: summary.albums,
                artists: summary.artists,
            })
        })
        .await
    }

    /// Delete every downloaded remote track. The library rows stay.
    pub async fn clear_download_cache(self: Arc<Self>) -> Result<CacheCleared, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let cfg = Config::load().unwrap_or_default();
            let cleared = koan_core::helpers::clear_download_cache(&db, &cfg);
            koan_core::helpers::requeue_cleared_downloads(&self.state, &self.tx);
            self.bump_library();
            Ok(CacheCleared {
                files: cleared.files,
                bytes: cleared.bytes,
            })
        })
        .await
    }

    /// Delete the downloaded copies of just these tracks. The library rows
    /// stay, and they fetch again on demand.
    pub async fn clear_downloads(
        self: Arc<Self>,
        track_ids: Vec<i64>,
    ) -> Result<CacheCleared, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let cleared = koan_core::helpers::clear_downloads_for(&db, &track_ids);
            koan_core::helpers::requeue_cleared_downloads(&self.state, &self.tx);
            self.bump_library();
            Ok(CacheCleared {
                files: cleared.files,
                bytes: cleared.bytes,
            })
        })
        .await
    }

    /// Fetch these tracks into the cache now, without queueing them.
    ///
    /// Downloads are normally a side effect of wanting to play something; this
    /// is for wanting the bytes on the machine and nothing else — before going
    /// somewhere without a server, most obviously. Tracks already downloaded
    /// are skipped, so asking twice costs nothing.
    ///
    /// The transfers get identities of their own rather than borrowing a queue
    /// item's, because there is no queue item: they appear in the download
    /// store and nowhere else.
    pub async fn download_to_cache(self: Arc<Self>, track_ids: Vec<i64>) -> Result<(), KoanError> {
        offload::offload(move || {
            let pending: Vec<(i64, koan_core::player::state::QueueItemId)> = track_ids
                .into_iter()
                .map(|id| (id, koan_core::player::state::QueueItemId::new()))
                .collect();
            koan_core::helpers::spawn_downloads(pending, self.tx.clone(), self.state.clone());
            Ok(())
        })
        .await
    }

    /// Which of these tracks have a downloaded copy. What a menu asks before
    /// deciding whether it is offering to fetch or to throw away.
    pub async fn downloaded_track_ids(
        self: Arc<Self>,
        track_ids: Vec<i64>,
    ) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::downloaded_of(&db.conn, &track_ids).map_err(db_err)
        })
        .await
    }

    /// Rescans every configured library folder. Minutes, on a large library.
    pub async fn scan(self: Arc<Self>, force: bool) -> Result<ScanSummary, KoanError> {
        offload::offload(move || self.scan_blocking(force, None)).await
    }

    /// `scan`, saying how far it has got.
    pub async fn scan_reporting(
        self: Arc<Self>,
        force: bool,
        reporter: Option<Arc<dyn ProgressReporter>>,
    ) -> Result<ScanSummary, KoanError> {
        offload::offload(move || self.scan_blocking(force, reporter)).await
    }

    /// Pull the remote library into the local database. Long and network-bound.
    /// `full` ignores the incremental cursor and re-walks every album.
    pub async fn sync_remote(self: Arc<Self>, full: bool) -> Result<SyncSummary, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let cfg = Config::load().unwrap_or_default();
            let client = koan_core::helpers::subsonic_client(&cfg).ok_or_else(|| {
                KoanError::BadArgument {
                    message: "no remote server configured".into(),
                }
            })?;

            let synced = koan_core::helpers::sync_remote(
                &db,
                &client,
                full,
                &cfg.remote.url,
                &cfg.remote.username,
            )
            .map_err(|e| KoanError::Database {
                message: e.to_string(),
            })?;

            self.bump_library();
            Ok(SyncSummary {
                artists: synced.library.artists_synced as u32,
                albums: synced.library.albums_synced as u32,
                tracks: synced.library.tracks_synced as u32,
                albums_failed: synced.library.albums_failed as u32,
                favourites_pushed: synced.favourites.pushed as u32,
                favourites_imported: synced.favourites.imported as u32,
                playlists_pulled: synced.playlists.pulled as u32,
                playlists_pushed: synced.playlists.pushed as u32,
            })
        })
        .await
    }

    /// Create a public share link on the remote server for these tracks.
    ///
    /// Only tracks the server knows about can go in it — the link points at the
    /// server, so a local-only file has nothing for it to point at. A mixed
    /// selection shares what it can; `skipped` says how much it left out.
    pub async fn create_share(
        self: Arc<Self>,
        track_ids: Vec<i64>,
        description: Option<String>,
    ) -> Result<Share, KoanError> {
        offload::offload(move || {
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
        })
        .await
    }

    /// Track IDs for an album or an artist, in running order. What the context
    /// menu actions resolve to before touching the queue.
    pub async fn track_ids(
        self: Arc<Self>,
        album_id: Option<i64>,
        artist_id: Option<i64>,
    ) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            Ok(self
                .tracks_blocking(album_id, artist_id, TrackSort::Album, 2000, 0)?
                .into_iter()
                .map(|t| t.id)
                .collect())
        })
        .await
    }

    // --- File organization -------------------------------------------------

    /// Named patterns from `[organize.patterns]`, sorted by name.
    ///
    /// Patterns are config, shared with the CLI and TUI, so this reads them
    /// rather than offering somewhere else to define them.
    pub async fn organize_patterns(self: Arc<Self>) -> Vec<OrganizePattern> {
        offload::offload(move || {
            let cfg = Config::load().unwrap_or_default().organize;
            let mut patterns: Vec<OrganizePattern> = cfg
                .patterns
                .iter()
                .map(|(name, pattern)| OrganizePattern {
                    name: name.clone(),
                    pattern: pattern.clone(),
                    is_default: cfg.default.as_deref() == Some(name.as_str()),
                })
                .collect();
            patterns.sort_by(|a, b| a.name.cmp(&b.name));
            patterns
        })
        .await
    }

    /// Store a named pattern in `config.toml`, replacing one of the same name.
    ///
    /// Writes the base config rather than the local overlay: patterns are a
    /// preference, not a machine fact, and the CLI and TUI read the same list.
    pub async fn save_organize_pattern(
        self: Arc<Self>,
        name: String,
        pattern: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let name = name.trim().to_string();
            if name.is_empty() || pattern.trim().is_empty() {
                return Err(KoanError::BadArgument {
                    message: "a pattern needs both a name and a format string".into(),
                });
            }
            // Parse it before storing it: a pattern that can't be evaluated would
            // sit in the config failing on every future run.
            koan_core::format::parse(&pattern).map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })?;
            Config::persist(|cfg| {
                cfg.organize.patterns.insert(name, pattern);
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
        })
        .await
    }

    /// Read a selection out of the library, ready to have patterns generated
    /// against it.
    ///
    /// This is the expensive half — database rows, album facts, a `stat` per
    /// file, and a tag read for anything the library has never seen — and it
    /// happens once per selection.
    /// `track_ids` of `None` means the whole library.
    pub async fn organize_selection(
        self: Arc<Self>,
        track_ids: Option<Vec<i64>>,
    ) -> Result<Arc<OrganizeSelection>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let requested = track_ids.as_ref().map(Vec::len);
            let inner =
                koan_core::organize::resolve(&db, track_ids.as_deref()).map_err(organize_err)?;
            Ok(Arc::new(OrganizeSelection { inner, requested }))
        })
        .await
    }

    /// Whether cover art, cue sheets and logs travel with the music.
    pub async fn organize_moves_ancillary(self: Arc<Self>) -> bool {
        offload::offload(move || Config::load().unwrap_or_default().organize.move_ancillary).await
    }

    /// Remember the choice. Written to `config.toml`, so the CLI and TUI
    /// organize the same way the app just did.
    pub async fn set_organize_moves_ancillary(
        self: Arc<Self>,
        enabled: bool,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            Config::persist(|cfg| cfg.organize.move_ancillary = enabled).map_err(|e| {
                KoanError::BadArgument {
                    message: e.to_string(),
                }
            })
        })
        .await
    }

    /// What `pattern` would do to these tracks. Touches nothing.
    ///
    /// `track_ids` of `None` means the whole library. `base_dir` picks which
    /// library folder the pattern's relative paths hang off; `None` uses the
    /// first configured one, matching the CLI. Resolves a destination per file.
    pub async fn organize_preview(
        self: Arc<Self>,
        pattern: String,
        track_ids: Option<Vec<i64>>,
        base_dir: Option<String>,
    ) -> Result<OrganizePlan, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let base = base_dir.map(PathBuf::from);
            let result = match &track_ids {
                Some(ids) => koan_core::organize::preview_for_tracks(
                    &db,
                    ids,
                    &pattern,
                    base.as_deref(),
                    true,
                ),
                None => koan_core::organize::preview(&db, &pattern, base.as_deref(), true),
            }
            .map_err(organize_err)?;
            Ok(OrganizePlan::build(
                result,
                track_ids.as_ref().map(Vec::len),
            ))
        })
        .await
    }

    /// Carry out the moves, then point the queue at where the files went.
    ///
    /// The rename and the database rows land together, and playback survives it
    /// — a Unix rename keeps the open descriptor. Destructive: run it only for
    /// a plan the user has seen.
    pub async fn organize_execute(
        self: Arc<Self>,
        pattern: String,
        track_ids: Option<Vec<i64>>,
        base_dir: Option<String>,
    ) -> Result<OrganizePlan, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let base = base_dir.map(PathBuf::from);
            let result = match &track_ids {
                Some(ids) => {
                    koan_core::organize::execute_for_tracks(&db, ids, &pattern, base.as_deref())
                }
                None => koan_core::organize::execute(&db, &pattern, base.as_deref()),
            }
            .map_err(organize_err)?;
            self.follow_moved_files(&result);
            self.bump_library();
            Ok(OrganizePlan::build(
                result,
                track_ids.as_ref().map(Vec::len),
            ))
        })
        .await
    }

    /// Index files from anywhere into the library, and return their track IDs.
    ///
    /// This is what a drop from Finder lands on: the files are read for tags,
    /// given library rows where they sit, and handed back as IDs the caller can
    /// queue. They are not moved — organize is what puts them under a library
    /// folder, and it can only do that once they have rows. Directories are
    /// walked recursively. Tag-bound, so it is proportional to the selection.
    pub async fn import_files(
        self: Arc<Self>,
        paths: Vec<String>,
    ) -> Result<ImportSummary, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            let result = koan_core::index::scanner::import_paths(&db, &paths);
            self.bump_library();
            Ok(ImportSummary {
                track_ids: result.track_ids,
                added: result.added as u32,
                updated: result.updated as u32,
                errors: result
                    .errors
                    .into_iter()
                    .map(|(p, e)| format!("{}: {e}", p.display()))
                    .collect(),
            })
        })
        .await
    }

    /// Where the library folders point. Shown in settings.
    pub async fn library_folders(self: Arc<Self>) -> Vec<String> {
        offload::offload(move || {
            Config::cached()
                .library
                .folders
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        })
        .await
    }
}

/// A resolved selection, held by the caller so a pattern can be generated
/// against it many times without re-reading anything.
///
/// The split is the point. `generate` is pure string work and runs on every
/// keystroke; `check` is the filesystem pass and runs once the typing settles.
/// Neither can change what the other decided, so the fast answer is never
/// wrong — only less complete.
#[derive(uniffi::Object)]
pub struct OrganizeSelection {
    inner: koan_core::organize::ResolvedSelection,
    /// How many tracks were asked for, so the shortfall can be reported.
    requested: Option<usize>,
}

#[uniffi::export]
impl OrganizeSelection {
    /// Turn the pattern into destinations. **Touches no files** — the cost is
    /// a destination per track, which is why it is off-thread like everything
    /// else rather than resolved inline as someone types.
    pub async fn generate(self: Arc<Self>, pattern: String, base_dir: String) -> OrganizePlan {
        offload::offload(move || {
            let result = koan_core::organize::generate(&self.inner, &pattern, Path::new(&base_dir));
            OrganizePlan::build(result, self.requested)
        })
        .await
    }

    /// Generate, then ask the disk the two questions generation cannot answer:
    /// which destinations are already occupied, and what ancillary files travel
    /// with each move. A `stat` per file and a directory read per source
    /// folder, so it runs once the typing settles rather than per keystroke.
    pub async fn check(
        self: Arc<Self>,
        pattern: String,
        base_dir: String,
        move_ancillary: bool,
    ) -> OrganizePlan {
        offload::offload(move || {
            let mut result =
                koan_core::organize::generate(&self.inner, &pattern, Path::new(&base_dir));
            koan_core::organize::check_against_disk(&mut result, move_ancillary);
            OrganizePlan::build(result, self.requested)
        })
        .await
    }

    /// How many files resolved to something local. Fewer than were asked for
    /// means the rest are remote-only or gone from disk.
    pub fn count(&self) -> u32 {
        self.inner.len() as u32
    }
}

// --- Internals -------------------------------------------------------------

impl KoanEngine {
    /// Watch shared state and publish what changes.
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
                let mut last_signature: Option<(PlaybackState, Option<String>, u64)> = None;
                let mut last_downloads: Vec<DownloadProgress> = Vec::new();
                let mut last_store = u64::MAX;
                // Temporary: answers "is the watcher actually emitting?" from
                // the log rather than from reasoning about it.
                let mut emitted_progress: u64 = 0;
                let mut emitted_store: u64 = 0;
                let mut emitted_position: u64 = 0;
                let mut ticks: u64 = 0;
                // Starts where the engine starts, so a launch is not announced
                // as a change to a library nobody has read yet.
                let mut last_library = 0u64;

                loop {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let Some(engine) = engine.upgrade() else {
                        return; // Engine dropped; so is the app.
                    };
                    // Nobody listening is not a reason to stop watching: a
                    // client can start a loop at any point and expects the
                    // next change, not the next change after it re-subscribes.
                    let publish = |event| {
                        let _ = engine.events.send(event);
                    };

                    let state = engine.state.playback_state();
                    let cursor = engine.state.cursor().map(|c| c.0.to_string());
                    // The seekable extent belongs in the signature: it moves
                    // while a track downloads without the state or the cursor
                    // moving, and a client draws a boundary from it. Rounded to
                    // the second, because it is drawn as a bar a few hundred
                    // pixels wide and nothing finer is visible. It stops moving
                    // the moment the download lands, so this costs a snapshot
                    // per tick only while one is in flight.
                    let seekable = engine.state.seekable_ms() / 1000;
                    let signature = (state, cursor, seekable);
                    if last_signature.as_ref() != Some(&signature) {
                        last_signature = Some(signature);
                        publish(PlayerEvent::PlaybackChanged {
                            now_playing: engine.now_playing_blocking(),
                        });
                    }

                    let version = engine.state.playlist_version();
                    if version != last_version {
                        last_version = version;
                        publish(PlayerEvent::QueueChanged { version });
                    }

                    // Progress moves without the version moving, so it gets its
                    // own event — one per tick while bytes are landing, and one
                    // empty list when the last transfer ends.
                    let downloads: Vec<DownloadProgress> = engine
                        .state
                        .downloads_in_flight()
                        .into_iter()
                        .map(|(id, done, total)| DownloadProgress {
                            queue_item_id: id.0.to_string(),
                            progress: (total > 0)
                                .then(|| (done as f64 / total as f64).clamp(0.0, 1.0)),
                        })
                        .collect();
                    if downloads != last_downloads {
                        // A transfer leaving the set landed on disk, which wrote
                        // a cached path onto a library row. Nothing else says
                        // so — the download runs in koan-core, which has no
                        // notion of this version — and without it a track goes
                        // on reading as "on the server, not here" until
                        // something unrelated reloads the row.
                        let still_running: std::collections::HashSet<&str> =
                            downloads.iter().map(|d| d.queue_item_id.as_str()).collect();
                        if last_downloads
                            .iter()
                            .any(|d| !still_running.contains(d.queue_item_id.as_str()))
                        {
                            engine.bump_library();
                        }
                        last_downloads = downloads.clone();
                        emitted_progress += 1;
                        publish(PlayerEvent::DownloadsChanged { downloads });
                    }

                    // Ten a second, which is this loop's rate and fast enough
                    // that a figure on screen reacts as a transfer changes pace.
                    koan_core::remote::downloads::store().sample_rates();

                    let store_version = koan_core::remote::downloads::store().version();
                    if store_version != last_store {
                        last_store = store_version;
                        emitted_store += 1;
                        publish(PlayerEvent::DownloadStoreChanged {
                            version: store_version,
                        });
                    }

                    // Once every five seconds, and only while something is
                    // happening. Fifty ticks is the ceiling for each count.
                    ticks += 1;
                    if ticks.is_multiple_of(50)
                        && (emitted_progress > 0 || emitted_position > 0 || emitted_store > 0)
                    {
                        log::info!(
                            "watcher/5s: position {emitted_position}, downloads {emitted_progress}, \
                             store {emitted_store}, in flight {}",
                            koan_core::remote::downloads::store().active()
                        );
                        emitted_progress = 0;
                        emitted_store = 0;
                        emitted_position = 0;
                    }

                    let library = engine
                        .library_version
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if library != last_library {
                        last_library = library;
                        publish(PlayerEvent::LibraryChanged { version: library });
                    }

                    // Only while playing: a paused position doesn't move, and
                    // re-sending it would keep a transport bar redrawing.
                    if state == PlaybackState::Playing {
                        let position = engine.state.position_ms();
                        if position != last_position {
                            last_position = position;
                            emitted_position += 1;
                            publish(PlayerEvent::PositionChanged {
                                position_ms: position,
                            });
                        }
                    }
                }
            })
            .ok();
    }

    /// Rewrite the queue to point at where organize put the files.
    ///
    /// The database rows moved with the files, but the playlist is in memory
    /// and still holds the old paths — a queued track would fail to open on the
    /// next play. Only the player may mutate it, so this goes through the
    /// command channel like every other queue change.
    fn follow_moved_files(&self, result: &koan_core::organize::OrganizeResult) {
        let moved: std::collections::HashMap<&Path, &Path> = result
            .moves()
            .filter_map(|e| e.to.as_deref().map(|to| (e.from.as_path(), to)))
            .collect();
        if moved.is_empty() {
            return;
        }

        let (items, _) = self.state.snapshot_playlist();
        let updates: Vec<(QueueItemId, PathBuf)> = items
            .iter()
            .filter_map(|item| {
                moved
                    .get(item.path.as_path())
                    .map(|to| (item.id, to.to_path_buf()))
            })
            .collect();
        if !updates.is_empty() {
            let _ = self.send(PlayerCommand::UpdatePaths(updates));
        }
    }

    fn tracks_blocking(
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

    /// Say that the library's rows changed. The watcher turns this into a
    /// `LibraryChanged` event on its next tick.
    fn bump_library(&self) {
        self.library_version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn scan_blocking(
        &self,
        force: bool,
        reporter: Option<Arc<dyn ProgressReporter>>,
    ) -> Result<ScanSummary, KoanError> {
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
            cancel: Some(self.cancel_library_task.clone()),
        };
        // A cancel from a previous run must not stop this one before it starts.
        self.cancel_library_task
            .store(false, std::sync::atomic::Ordering::Relaxed);

        if let Some(reporter) = &reporter {
            reporter.started(koan_core::index::scanner::count_audio_files(
                &cfg.library.folders,
            ));
        }

        let done = std::sync::atomic::AtomicU64::new(0);
        for folder in &cfg.library.folders {
            let callback = |event: koan_core::index::scanner::ScanEvent| {
                let Some(reporter) = &reporter else { return };
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                // Reporting every file would be tens of thousands of trips over
                // the FFI and a redraw for each. Every 64 still looks live.
                if n.is_multiple_of(64) {
                    reporter.advanced(n, format!("{} — {}", event.artist, event.title));
                }
            };
            let hook: Option<&dyn Fn(koan_core::index::scanner::ScanEvent)> =
                reporter.as_ref().map(|_| &callback as _);

            let r = koan_core::index::scanner::scan_folder(&db, folder, opts.clone(), hook);
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
        self.bump_library();
        Ok(summary)
    }

    fn build() -> Result<Arc<Self>, KoanError> {
        init_logging();
        let db_path = config::db_path();
        // Fail fast on a broken library rather than after the audio threads exist.
        Database::open(&db_path).map_err(|e| KoanError::Database {
            message: e.to_string(),
        })?;

        // Before anything can start a download of its own, so this only ever
        // sees files left by a previous run.
        koan_core::helpers::sweep_partial_downloads(&Config::load().unwrap_or_default());

        let (state, _timeline, viz, tx) = Player::spawn();
        koan_core::radio::spawn_autoqueue(state.clone(), tx.clone(), db_path.clone());

        // Bumped by the background tasks below as well as by everything the UI
        // asks for, so a sync nobody asked for reaches a client the same way.
        let library_version = Arc::new(std::sync::atomic::AtomicU64::new(0));

        // Finishing is the interesting edge: rows landed while it ran, and the
        // moment it stops is the moment they are all there.
        let finished = {
            let version = library_version.clone();
            move |flag: &std::sync::atomic::AtomicBool, running: bool| {
                if !running && flag.swap(running, std::sync::atomic::Ordering::Relaxed) {
                    version.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    flag.store(running, std::sync::atomic::Ordering::Relaxed);
                }
            }
        };

        let auto_syncing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let flag = auto_syncing.clone();
            let finished = finished.clone();
            koan_core::helpers::spawn_auto_sync(db_path.clone(), move |running| {
                finished(&flag, running);
            });
        }

        // Local files are watched rather than synced on a timer: a folder that
        // has not changed costs nothing to notice, and one that has should show
        // up without being asked.
        let auto_scanning = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let flag = auto_scanning.clone();
            koan_core::helpers::spawn_library_watch(db_path.clone(), move |running| {
                finished(&flag, running);
            });
        }

        // Capacity, not a queue to drain: a client that falls behind drops the
        // events it missed, and the next one it does get is a complete answer.
        let (events, _) = tokio::sync::broadcast::channel(64);
        let engine = Arc::new(Self {
            state,
            tx,
            viz,
            db_path,
            events,
            auto_syncing,
            auto_scanning,
            cancel_library_task: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            library_version: library_version.clone(),
        });
        engine.spawn_watcher();
        Ok(engine)
    }

    fn now_playing_blocking(&self) -> NowPlaying {
        let info = self.state.track_info();
        let cursor = self.state.cursor();
        let play_state = self.state.playback_state();
        let entry = cursor
            .and_then(|cid| self.state.get_item(cid))
            .map(|item| QueueItem::from_cursor_item(&item, play_state));

        NowPlaying {
            state: play_state.into(),
            position_ms: self.state.position_ms(),
            duration_ms: self.state.duration_ms(),
            seekable_ms: self.state.seekable_ms(),
            queue_item_id: cursor.map(|c| c.0.to_string()),
            entry,
            format: info
                .as_ref()
                .map(|i| StreamFormat::of(i, self.state.output_sample_rate())),
            playlist_version: self.state.playlist_version(),
            radio_enabled: self.state.radio_mode(),
        }
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

    /// Whether the queue is still exactly this playlist.
    fn locked_to(&self, db: &Database, playlist_id: i64) -> bool {
        koan_core::playlists::queue_lock(db, &self.state)
            == Some(koan_core::playlists::QueueLock::Playlist(playlist_id))
    }

    /// Make the queue follow a playlist that has just been edited.
    ///
    /// Only when the queue was still exactly that playlist beforehand —
    /// `was_locked` is read *before* the edit lands, because afterwards the two
    /// no longer match and every queue would look diverged.
    ///
    /// The edit is applied to the queue rather than the queue being rebuilt
    /// from the playlist: entries that survived keep their queue items, and
    /// with them their ids, what has played, what is mid-download and what the
    /// cursor is pointing at.
    fn follow_playlist(&self, db: &Database, playlist_id: i64, was_locked: bool) {
        if !was_locked {
            return;
        }
        let Ok(entries) = queries::playlist_entries(&db.conn, playlist_id) else {
            return;
        };
        let (items, _) = self.state.snapshot_playlist();

        // Queue items whose entry has gone from the playlist.
        let live: std::collections::HashSet<i64> = entries.iter().map(|e| e.id).collect();
        let doomed: Vec<QueueItemId> = items
            .iter()
            .filter(|i| i.playlist_entry_id.is_none_or(|e| !live.contains(&e)))
            .map(|i| i.id)
            .collect();
        if !doomed.is_empty() {
            let _ = self.send(PlayerCommand::RemoveFromPlaylistBatch(doomed));
        }

        // Entries the queue has never seen.
        let known: std::collections::HashMap<i64, QueueItemId> = items
            .iter()
            .filter_map(|i| i.playlist_entry_id.map(|e| (e, i.id)))
            .collect();
        let missing: Vec<&queries::PlaylistEntry> = entries
            .iter()
            .filter(|e| !known.contains_key(&e.id))
            .collect();
        let mut added = std::collections::HashMap::new();
        if !missing.is_empty() {
            let track_ids: Vec<i64> = missing.iter().map(|e| e.track.id).collect();
            let (mut new_items, pending) = self.build_items(db, &track_ids);
            for (item, entry) in new_items.iter_mut().zip(&missing) {
                item.playlist_entry_id = Some(entry.id);
                added.insert(entry.id, item.id);
            }
            if !new_items.is_empty() {
                let _ = self.send(PlayerCommand::AddToPlaylist(new_items));
                self.start_downloads(pending);
            }
        }

        // Then the order, which is what puts the new arrivals in their places —
        // they were appended, because that is the only thing an add can do.
        let order: Vec<QueueItemId> = entries
            .iter()
            .filter_map(|e| known.get(&e.id).or_else(|| added.get(&e.id)).copied())
            .collect();
        if !order.is_empty() {
            let _ = self.send(PlayerCommand::ReorderPlaylist(order));
        }
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

/// A search term the user actually typed, or nothing. Whitespace is not a
/// filter, and neither is an empty box.
fn trimmed(search: &Option<String>) -> Option<&str> {
    search.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// The browser's sort as an order the database can apply.
fn album_order(sort: AlbumSort, seed: i64) -> queries::AlbumOrder {
    match sort {
        AlbumSort::RecentlyAdded => queries::AlbumOrder::RecentlyAdded,
        AlbumSort::Title => queries::AlbumOrder::Title,
        AlbumSort::Artist => queries::AlbumOrder::ArtistThenDate,
        AlbumSort::Year => queries::AlbumOrder::YearDesc,
        AlbumSort::Random => queries::AlbumOrder::Random(seed),
    }
}

/// Rank `texts` against `query`, best first, and return the indices of the top
/// `limit`. Shared by every fuzzy listing so they rank identically.
fn fuzzy_rank(texts: &[&str], query: &str, limit: u32) -> Vec<usize> {
    use nucleo::pattern::{CaseMatching, Normalization};
    use nucleo::{Config as NucleoConfig, Nucleo};

    let mut nucleo: Nucleo<u32> = Nucleo::new(NucleoConfig::DEFAULT, Arc::new(|| {}), None, 1);
    let injector = nucleo.injector();
    for (i, text) in texts.iter().enumerate() {
        let text = text.to_string();
        injector.push(i as u32, |_val, cols| {
            cols[0] = text.into();
        });
    }

    nucleo
        .pattern
        .reparse(0, query, CaseMatching::Smart, Normalization::Smart, false);
    for _ in 0..20 {
        nucleo.tick(10);
    }

    let snap = nucleo.snapshot();
    let count = (snap.matched_item_count() as usize).min(limit as usize);
    (0..count as u32)
        .filter_map(|i| snap.get_matched_item(i).map(|item| *item.data as usize))
        .collect()
}

fn db_err(e: impl std::fmt::Display) -> KoanError {
    KoanError::Database {
        message: e.to_string(),
    }
}

/// A pattern that can't be resolved, a library folder that isn't configured and
/// a file that won't move are all things the user can act on, so they come back
/// as bad arguments rather than as an opaque failure. Only the database going
/// wrong is out of their hands.
fn organize_err(e: koan_core::organize::OrganizeError) -> KoanError {
    use koan_core::organize::OrganizeError as E;
    let message = e.to_string();
    match e {
        E::Db(_) | E::Sqlite(_) => KoanError::Database { message },
        _ => KoanError::BadArgument { message },
    }
}

fn fav_err(e: rusqlite::Error) -> KoanError {
    KoanError::Database {
        message: e.to_string(),
    }
}

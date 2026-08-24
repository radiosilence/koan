mod helpers;
mod jobs;
mod loaders;
mod mutations;
mod queries;
mod server;
mod subscriptions;
mod types;

use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Schema};
use crossbeam_channel::{Sender, TrySendError};
use koan_core::audio::viz::VizSnapshot;
use koan_core::db::connection::Database;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueItemId, SharedPlayerState};
use uuid::Uuid;

use koan_core::auth::Role;
use loaders::DbLoader;
use mutations::MutationRoot;
use queries::QueryRoot;
pub use server::{
    ApiServerOpts, cmd_serve, cmd_serve_daemon, execute_in_process, start_api_background,
};
use subscriptions::SubscriptionRoot;

use crate::auth::AuthUser;

// ---------------------------------------------------------------------------
// Connection pool
// ---------------------------------------------------------------------------

/// How long the dataloader gathers keys before running a batch.
///
/// The default 1ms closes the window while a wide selection set is still
/// registering keys, splitting one query into several.
const BATCH_WINDOW: Duration = Duration::from_millis(10);

/// How long a resolver waits for a free connection before giving up. Long
/// enough to ride out a scan chunk, short enough that a wedged writer surfaces
/// as an error rather than a hung request.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// A fixed set of SQLite connections handed out per query, not per field.
///
/// `Database::open` runs the DDL batch, the migrations and a WAL checkpoint, so
/// opening one per resolver field costs tens of statements before the actual
/// query runs. Connections are opened lazily up to `max` and returned on drop.
/// Deliberately not a single `Mutex<Connection>`: WAL gives concurrent readers,
/// and one slow query must not block every other client.
struct DbPool {
    path: PathBuf,
    /// Returned connections wait here. Bounded at `max`, so `try_send` on the
    /// return path cannot block or fail for lack of room.
    idle_tx: Sender<Database>,
    idle_rx: crossbeam_channel::Receiver<Database>,
    /// Connections in existence (idle plus checked out).
    live: AtomicUsize,
    /// Total connections ever opened. Instrumentation only — the fan-out tests
    /// assert this stays bounded as a query's breadth grows.
    opens: AtomicUsize,
    /// Dataloader batches run. Instrumentation only — the N+1 tests assert one
    /// batch serves a whole selection set.
    batches: AtomicUsize,
    /// Whether the schema and migrations have been applied to this path.
    initialised: AtomicBool,
    max: usize,
}

impl DbPool {
    fn new(path: PathBuf) -> Arc<Self> {
        // SQLite readers scale with cores; past that they only queue on the OS.
        let max = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(4, 16);
        let (idle_tx, idle_rx) = crossbeam_channel::bounded(max);
        Arc::new(Self {
            path,
            idle_tx,
            idle_rx,
            live: AtomicUsize::new(0),
            opens: AtomicUsize::new(0),
            batches: AtomicUsize::new(0),
            initialised: AtomicBool::new(false),
            max,
        })
    }

    /// The first connection applies the schema; every later one skips it.
    fn open_new(&self) -> Result<Database, koan_core::db::connection::DbError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        if self.initialised.load(Ordering::Acquire) {
            Database::open_existing(&self.path)
        } else {
            let db = Database::open(&self.path)?;
            self.initialised.store(true, Ordering::Release);
            Ok(db)
        }
    }

    fn acquire(self: &Arc<Self>) -> async_graphql::Result<PooledDb> {
        if let Ok(db) = self.idle_rx.try_recv() {
            return Ok(PooledDb {
                db: Some(db),
                pool: self.clone(),
            });
        }

        let mut live = self.live.load(Ordering::Relaxed);
        while live < self.max {
            match self.live.compare_exchange_weak(
                live,
                live + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return match self.open_new() {
                        Ok(db) => Ok(PooledDb {
                            db: Some(db),
                            pool: self.clone(),
                        }),
                        Err(e) => {
                            self.live.fetch_sub(1, Ordering::AcqRel);
                            Err(internal_error("db open", e))
                        }
                    };
                }
                Err(actual) => live = actual,
            }
        }

        self.idle_rx
            .recv_timeout(ACQUIRE_TIMEOUT)
            .map(|db| PooledDb {
                db: Some(db),
                pool: self.clone(),
            })
            .map_err(|_| async_graphql::Error::new("database busy"))
    }
}

/// A connection checked out of the pool, returned when the guard drops.
struct PooledDb {
    db: Option<Database>,
    pool: Arc<DbPool>,
}

impl Deref for PooledDb {
    type Target = Database;

    fn deref(&self) -> &Database {
        self.db.as_ref().expect("connection taken before drop")
    }
}

impl Drop for PooledDb {
    fn drop(&mut self) {
        if let Some(db) = self.db.take()
            && let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) =
                self.pool.idle_tx.try_send(db)
        {
            self.pool.live.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

// ---------------------------------------------------------------------------
// DB handle wrapper (so we can put it in Context)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DbHandle {
    pool: Arc<DbPool>,
}

impl DbHandle {
    fn new(path: PathBuf) -> Self {
        Self {
            pool: DbPool::new(path),
        }
    }

    fn acquire(&self) -> async_graphql::Result<PooledDb> {
        self.pool.acquire()
    }

    /// A connection outside the pool, for work that runs for minutes and must
    /// not deny a connection to request-path resolvers.
    fn open_detached(&self) -> Result<Database, koan_core::db::connection::DbError> {
        self.pool.open_new()
    }

    fn note_batch(&self) {
        self.pool.batches.fetch_add(1, Ordering::Relaxed);
    }

    /// Connections opened since the schema was built.
    #[cfg_attr(not(test), allow(dead_code))]
    fn open_count(&self) -> usize {
        self.pool.opens.load(Ordering::Relaxed)
    }

    /// Dataloader batches run since the schema was built.
    #[cfg_attr(not(test), allow(dead_code))]
    fn batch_count(&self) -> usize {
        self.pool.batches.load(Ordering::Relaxed)
    }
}

/// Run a rusqlite closure on the blocking pool with a pooled connection.
///
/// rusqlite is blocking start to finish. Calling it inline in an `async fn`
/// parks a tokio worker for the duration, and enough of those at once starve
/// the runtime — including the `ReaderStream`s feeding in-flight audio.
async fn with_db<T, F>(ctx: &Context<'_>, f: F) -> async_graphql::Result<T>
where
    F: FnOnce(&Database) -> async_graphql::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let handle = ctx.data::<DbHandle>()?.clone();
    blocking(move || {
        let db = handle.acquire()?;
        f(&db)
    })
    .await
}

/// Run any other blocking work (HTTP fetches, file decoding, tag reads) off the
/// async workers.
async fn blocking<T, F>(f: F) -> async_graphql::Result<T>
where
    F: FnOnce() -> async_graphql::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| internal_error("blocking task", e))?
}

/// Log the detail, return a generic message.
///
/// SQLite errors quote the offending statement and filesystem errors quote
/// absolute paths — a map of the host handed to whoever asked.
pub(super) fn internal_error(context: &str, e: impl std::fmt::Display) -> async_graphql::Error {
    log::error!("graphql {}: {}", context, e);
    async_graphql::Error::new("internal error")
}

// ---------------------------------------------------------------------------
// Schema builder
// ---------------------------------------------------------------------------

pub type KoanSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub fn build_schema(
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    db_path: PathBuf,
    viz: Option<Arc<VizSnapshot>>,
) -> KoanSchema {
    build_schema_with(DbHandle::new(db_path), state, cmd_tx, viz)
}

fn build_schema_with(
    handle: DbHandle,
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    viz: Option<Arc<VizSnapshot>>,
) -> KoanSchema {
    // Batching only, no caching: a schema-wide loader outlives the request, and
    // favourites and library contents change under it.
    let loader = DataLoader::new(DbLoader::new(handle.clone()), tokio::spawn).delay(BATCH_WINDOW);
    loader.enable_all_cache(false);

    let mut builder = Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .data(handle)
        .data(loader)
        .data(jobs::JobRegistry::default())
        .data(state)
        .data(cmd_tx);
    if let Some(viz) = viz {
        builder = builder.data(viz);
    }
    // A single nested query can otherwise fan out across the whole library and
    // pin the process for minutes.
    builder.limit_depth(12).limit_complexity(2000).finish()
}

// ---------------------------------------------------------------------------
// Shared helpers used by queries + mutations
// ---------------------------------------------------------------------------

fn parse_queue_item_id(s: &str) -> async_graphql::Result<QueueItemId> {
    Uuid::parse_str(s)
        .map(QueueItemId)
        .map_err(|e| async_graphql::Error::new(format!("invalid queue item ID '{}': {}", s, e)))
}

/// How long to wait for room on the player command channel.
///
/// The channel is bounded(16) and the player can sit inside `start_playback`
/// for about a second during a device sample-rate change. A blocking `send`
/// there parks a tokio worker; this gives the player a moment to drain and
/// then reports back rather than holding the thread.
const CMD_SEND_TIMEOUT: Duration = Duration::from_millis(250);

fn send_cmd(ctx: &Context<'_>, cmd: PlayerCommand) -> async_graphql::Result<()> {
    let tx = ctx.data::<Sender<PlayerCommand>>()?;
    send_cmd_via(tx, cmd)
}

fn send_cmd_via(tx: &Sender<PlayerCommand>, cmd: PlayerCommand) -> async_graphql::Result<()> {
    tx.send_timeout(cmd, CMD_SEND_TIMEOUT)
        .map_err(|_| async_graphql::Error::new("player busy — command not accepted"))
}

/// Extract the authenticated user from GraphQL context.
/// Returns anonymous admin if no user is present (auth disabled or in-process).
fn get_auth_user(ctx: &Context<'_>) -> AuthUser {
    ctx.data::<AuthUser>()
        .cloned()
        .unwrap_or_else(|_| AuthUser::anonymous_admin())
}

/// Check that the current user has at least the required role.
/// Returns an error suitable for GraphQL if the check fails.
fn require_role(ctx: &Context<'_>, required: Role) -> async_graphql::Result<()> {
    let user = get_auth_user(ctx);
    if user.role.has_permission(required) {
        Ok(())
    } else {
        Err(async_graphql::Error::new(format!(
            "forbidden: requires {} role, you have {}",
            required, user.role
        )))
    }
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

        let schema = build_schema(state, tx, db_path, None);
        (schema, rx, tmp)
    }

    /// Same schema, but keeping the `DbHandle` so tests can read its counters.
    fn instrumented_schema() -> (
        KoanSchema,
        DbHandle,
        crossbeam_channel::Receiver<PlayerCommand>,
        TempDir,
    ) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();

        let state = SharedPlayerState::new();
        let ch = CommandChannel::new();
        let handle = DbHandle::new(db_path);
        let schema = build_schema_with(handle.clone(), state, ch.tx.clone(), None);
        (schema, handle, ch.rx.clone(), tmp)
    }

    /// Seed a library on one connection — the per-track helper opens its own.
    fn seed_library(db_path: &std::path::Path, artists: usize, albums: usize, tracks: usize) {
        let db = Database::open(db_path).unwrap();
        for a in 0..artists {
            for al in 0..albums {
                for t in 0..tracks {
                    let meta = queries::TrackMeta {
                        title: format!("Track {:03}-{}-{:03}", a, al, t),
                        artist: format!("Artist {:03}", a),
                        album_artist: Some(format!("Artist {:03}", a)),
                        album: format!("Album {:03}-{}", a, al),
                        track_number: Some(t as i32),
                        disc: Some(1),
                        date: Some("2024".into()),
                        genre: Some("Electronic".into()),
                        duration_ms: Some(240_000),
                        path: Some(format!("/tmp/koan-test/{}/{}/{}.flac", a, al, t)),
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
                        album_remote_id: None,
                        artist_remote_id: None,
                        mbid: None,
                        album_added_at: None,
                        label: None,
                    };
                    queries::upsert_track(&db.conn, &meta).unwrap();
                }
            }
        }
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
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            album_added_at: None,
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
    async fn enqueue_mutation_adds_to_queue() {
        let (schema, rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");

        // Insert a track into the DB.
        let track_id = insert_test_track(&db_path, "Windowlicker", "Aphex Twin", "Windowlicker EP");

        // Execute the addToQueue mutation.
        let query = format!(
            "mutation {{ addToQueue(trackIds: [{}]) {{ ok message addedCount queueItemIds }} }}",
            track_id
        );
        let resp = schema.execute(&query).await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);

        let data = resp.data.into_json().unwrap();
        assert_eq!(data["addToQueue"]["ok"], true);
        assert_eq!(data["addToQueue"]["addedCount"], 1);

        let queue_ids = data["addToQueue"]["queueItemIds"].as_array().unwrap();
        assert_eq!(queue_ids.len(), 1, "should return one queue item ID");

        // Verify the PlayerCommand was sent through the channel.
        // The mutation sends AddToPlaylist and then Play (auto-play when stopped).
        let cmd = rx.try_recv().unwrap();
        match cmd {
            PlayerCommand::AddToPlaylist(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].title, "Windowlicker");
                assert_eq!(items[0].artist, "Aphex Twin");
                assert_eq!(items[0].album, "Windowlicker EP");
            }
            other => panic!("expected AddToPlaylist, got {:?}", other),
        }

        // Auto-play command should follow.
        let play_cmd = rx.try_recv().unwrap();
        assert!(
            matches!(play_cmd, PlayerCommand::Play(_)),
            "expected Play command for auto-play, got {:?}",
            play_cmd
        );
    }

    #[tokio::test]
    async fn replace_queue_mutation_clears_and_enqueues() {
        let (schema, rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");

        let id1 = insert_test_track(&db_path, "Track A", "Artist", "Album");
        let id2 = insert_test_track(&db_path, "Track B", "Artist", "Album");

        let query = format!(
            "mutation {{ replaceQueue(trackIds: [{}, {}]) {{ ok addedCount queueItemIds }} }}",
            id1, id2
        );
        let resp = schema.execute(&query).await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);

        let data = resp.data.into_json().unwrap();
        assert_eq!(data["replaceQueue"]["addedCount"], 2);

        // One command, not clear-then-add-then-play: three commands down a
        // bounded channel means the first track starts before the cursor lands
        // on the one that was asked for.
        match rx.try_recv().unwrap() {
            PlayerCommand::ReplacePlaylist { items, start } => {
                assert_eq!(items.len(), 2);
                assert_eq!(start, 0, "defaults to the first track");
            }
            other => panic!("expected ReplacePlaylist, got {:?}", other),
        }
        assert!(rx.try_recv().is_err(), "no follow-up commands");
    }

    #[tokio::test]
    async fn replace_queue_starts_where_it_was_asked_to() {
        let (schema, rx, tmp) = test_schema();
        let db_path = tmp.path().join("test.db");

        let id1 = insert_test_track(&db_path, "Track A", "Artist", "Album");
        let id2 = insert_test_track(&db_path, "Track B", "Artist", "Album");

        let resp = schema
            .execute(&format!(
                "mutation {{ replaceQueue(trackIds: [{}, {}], startAt: 1) {{ ok }} }}",
                id1, id2
            ))
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);

        match rx.try_recv().unwrap() {
            PlayerCommand::ReplacePlaylist { start, .. } => assert_eq!(start, 1),
            other => panic!("expected ReplacePlaylist, got {:?}", other),
        }
    }

    // ---- Phase 1 tests: queue snapshot, viz, config, playlist version, subscriptions ----

    #[tokio::test]
    async fn queue_snapshot_has_version_and_status() {
        let (schema, _rx, _tmp) = test_schema();

        let resp = schema
            .execute("{ queue { version entries { queueItemId status } hasPlaying queueCount } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        // Empty queue.
        assert_eq!(data["queue"]["version"], 0);
        assert_eq!(data["queue"]["entries"].as_array().unwrap().len(), 0);
        assert_eq!(data["queue"]["hasPlaying"], false);
        assert_eq!(data["queue"]["queueCount"], 0);
    }

    #[tokio::test]
    async fn queue_entries_have_status_and_download_progress() {
        use koan_core::player::state::{LoadState, PlaylistItem};

        // Build schema with a shared state we can manipulate directly.
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();

        let state = SharedPlayerState::new();
        let ch = CommandChannel::new();
        let schema = build_schema(state.clone(), ch.tx.clone(), db_path, None);

        // Directly add items to the playlist (simulating what the player thread does).
        let item = PlaylistItem {
            id: QueueItemId::new(),
            db_id: None,
            path: std::path::PathBuf::from("/tmp/test/windowlicker.flac"),
            title: "Windowlicker".to_string(),
            artist: "Aphex Twin".to_string(),
            album_artist: "Aphex Twin".to_string(),
            album: "Windowlicker EP".to_string(),
            year: None,
            codec: Some("FLAC".to_string()),
            track_number: Some(1),
            disc: Some(1),
            duration_ms: Some(240000),
            load_state: LoadState::Ready,
        };
        state.add_items(vec![item]);

        // Query the queue — should have one entry with QUEUED status.
        let resp = schema
            .execute(
                "{ queue { version entries { queueItemId title status downloadProgress { downloaded total } isCurrent } finishedCount } }",
            )
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let entries = data["queue"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["title"], "Windowlicker");
        // Without a cursor set, all entries are QUEUED.
        assert_eq!(entries[0]["status"], "QUEUED");
        assert_eq!(entries[0]["isCurrent"], false);
        // Local track — no download progress.
        assert!(entries[0]["downloadProgress"].is_null());
    }

    #[tokio::test]
    async fn viz_frame_returns_none_without_viz() {
        let (schema, _rx, _tmp) = test_schema();

        let resp = schema
            .execute("{ vizFrame { spectrum peaks vuLevels beatEnergy } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert!(data["vizFrame"].is_null());
    }

    #[tokio::test]
    async fn viz_frame_returns_data_with_viz() {
        // Build schema with a VizSnapshot.
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();

        let state = SharedPlayerState::new();
        let ch = CommandChannel::new();
        let viz = koan_core::audio::viz::VizSnapshot::new();

        // Write some test data.
        let mut spectrum = [0.0f32; 48];
        spectrum[0] = 0.75;
        viz.write(koan_core::audio::viz::VizFrame {
            spectrum,
            peaks: [0.0; 48],
            vu_levels: [0.42, 0.38],
            beat_energy: 0.6,
            timestamp: std::time::Instant::now(),
            waveform: Vec::new(),
        });

        let schema = build_schema(state, ch.tx.clone(), db_path, Some(viz));

        let resp = schema
            .execute("{ vizFrame { spectrum peaks vuLevels beatEnergy waveform } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let frame = &data["vizFrame"];
        assert!(!frame.is_null());
        let spectrum = frame["spectrum"].as_array().unwrap();
        assert_eq!(spectrum.len(), 48);
        assert!((spectrum[0].as_f64().unwrap() - 0.75).abs() < 0.01);
        let vu = frame["vuLevels"].as_array().unwrap();
        assert_eq!(vu.len(), 2);
        assert!((vu[0].as_f64().unwrap() - 0.42).abs() < 0.01);
        assert!((frame["beatEnergy"].as_f64().unwrap() - 0.6).abs() < 0.01);
        // Waveform empty — we didn't request includeWaveform.
        let waveform = frame["waveform"].as_array().unwrap();
        assert!(waveform.is_empty());
    }

    #[tokio::test]
    async fn config_query() {
        let (schema, _rx, _tmp) = test_schema();

        let resp = schema
            .execute(
                "{ config { libraryFolders replaygainMode targetFps artSize remoteEnabled graphqlPort } }",
            )
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let cfg = &data["config"];
        // Defaults from Config::default().
        assert!(cfg["libraryFolders"].is_array());
        assert!(cfg["targetFps"].as_i64().unwrap() > 0);
        assert!(cfg["artSize"].as_i64().unwrap() > 0);
    }

    #[tokio::test]
    async fn playlist_version_query() {
        let (schema, _rx, _tmp) = test_schema();

        let resp = schema.execute("{ playlistVersion }").await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["playlistVersion"], 0);
    }

    #[tokio::test]
    async fn subscription_types_in_schema() {
        // Verify that subscriptions are registered by introspecting the schema.
        let (schema, _rx, _tmp) = test_schema();

        let resp = schema
            .execute("{ __schema { subscriptionType { fields { name } } } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let fields = data["__schema"]["subscriptionType"]["fields"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = fields.iter().filter_map(|f| f["name"].as_str()).collect();
        assert!(
            names.contains(&"nowPlaying"),
            "missing nowPlaying subscription"
        );
        assert!(
            names.contains(&"queueUpdated"),
            "missing queueUpdated subscription"
        );
        assert!(names.contains(&"vizFrame"), "missing vizFrame subscription");
    }
    // -- Fan-out and pagination --

    #[tokio::test]
    async fn nested_fan_out_opens_a_bounded_number_of_connections() {
        let (schema, handle, _rx, tmp) = instrumented_schema();
        seed_library(&tmp.path().join("test.db"), 10, 3, 4);

        let before = handle.open_count();
        let resp = schema
            .execute(
                "{ artists(first: 10) { edges { node { name albumCount trackCount \
                   albums(first: 10) { edges { node { title trackCount totalDurationMs \
                   tracks(first: 10) { edges { node { title isFavourite } } } } } } } } } }",
            )
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);

        let data = resp.data.into_json().unwrap();
        let artists = data["artists"]["edges"].as_array().unwrap();
        assert_eq!(artists.len(), 10, "query did not actually fan out");
        assert_eq!(artists[0]["node"]["albumCount"], 3);
        assert_eq!(artists[0]["node"]["trackCount"], 12);

        // Before pooling this was one full `Database::open` — DDL batch,
        // migrations and a WAL checkpoint — per resolver field.
        let opened = handle.open_count() - before;
        assert!(opened <= 16, "opened {} connections", opened);
    }

    #[tokio::test]
    async fn is_favourite_batches_into_one_query() {
        let (schema, handle, _rx, tmp) = instrumented_schema();
        seed_library(&tmp.path().join("test.db"), 1, 1, 100);

        let before = handle.batch_count();
        let resp = schema
            .execute("{ tracks(first: 100) { edges { node { title isFavourite } } } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["tracks"]["edges"].as_array().unwrap().len(), 100);

        // The invariant is that the query count does not scale with the row
        // count: without the dataloader this was one full `favourites` scan per
        // track. The dataloader's gather window can close more than once under
        // load, so assert the property rather than an exact batch count.
        let batches = handle.batch_count() - before;
        assert!(
            batches < 10,
            "{batches} batches for 100 tracks — expected a handful, not one per row"
        );
    }

    #[tokio::test]
    async fn tracks_without_first_returns_the_default_page() {
        let (schema, _handle, _rx, tmp) = instrumented_schema();
        seed_library(
            &tmp.path().join("test.db"),
            1,
            1,
            helpers::DEFAULT_PAGE + 20,
        );

        let resp = schema
            .execute("{ tracks { edges { node { title } } pageInfo { hasNextPage } } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(
            data["tracks"]["edges"].as_array().unwrap().len(),
            helpers::DEFAULT_PAGE
        );
        assert_eq!(data["tracks"]["pageInfo"]["hasNextPage"], true);
    }

    #[tokio::test]
    async fn first_is_clamped_to_the_maximum_page() {
        let (schema, _handle, _rx, tmp) = instrumented_schema();
        seed_library(&tmp.path().join("test.db"), 1, 1, 10);

        let resp = schema
            .execute("{ tracks(first: 100000) { edges { node { title } } } }")
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["tracks"]["edges"].as_array().unwrap().len(), 10);
    }

    #[tokio::test]
    async fn negative_first_is_empty_not_a_panic() {
        let (schema, _handle, _rx, tmp) = instrumented_schema();
        seed_library(&tmp.path().join("test.db"), 2, 1, 3);

        for query in [
            "{ tracks(first: -1) { edges { node { title } } } }",
            "{ artists(first: -1) { edges { node { name } } } }",
            "{ albums(first: -1) { edges { node { title } } } }",
        ] {
            let resp = schema.execute(query).await;
            assert!(resp.errors.is_empty(), "{}: {:?}", query, resp.errors);
        }
    }

    #[tokio::test]
    async fn sort_arguments_are_honoured() {
        let (schema, _handle, _rx, tmp) = instrumented_schema();
        seed_library(&tmp.path().join("test.db"), 3, 1, 2);

        let resp = schema
            .execute(
                "{ tracks(first: 100, sortBy: TITLE, sortDir: DESC) { edges { node { title } } } }",
            )
            .await;
        assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
        let data = resp.data.into_json().unwrap();
        let titles: Vec<String> = data["tracks"]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["node"]["title"].as_str().unwrap().to_string())
            .collect();
        let mut sorted = titles.clone();
        sorted.sort();
        sorted.reverse();
        assert_eq!(titles, sorted, "sortBy/sortDir were ignored");
    }

    /// A blocking resolver must not hold the runtime. On a single-threaded
    /// runtime, anything running inline would freeze every other task for the
    /// whole query — which is what stalled in-flight audio streams.
    #[test]
    fn a_slow_query_does_not_stall_other_tasks() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let (schema, _handle, _rx, tmp) = instrumented_schema();
        seed_library(&tmp.path().join("test.db"), 20, 5, 8);

        rt.block_on(async move {
            let ticks = Arc::new(AtomicUsize::new(0));
            let counter = ticks.clone();
            let ticker = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            });

            // fuzzySearch loads the library and builds a whole Nucleo matcher.
            let mut running = Vec::new();
            for _ in 0..4 {
                let schema = schema.clone();
                running.push(tokio::spawn(async move {
                    schema
                        .execute("{ fuzzySearch(query: \"track\") { id } }")
                        .await
                }));
            }
            for handle in running {
                let resp = handle.await.unwrap();
                assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
            }

            ticker.abort();
            assert!(
                ticks.load(Ordering::Relaxed) >= 2,
                "no other task ran while the queries were in flight"
            );
        });
    }
}

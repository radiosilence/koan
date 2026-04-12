mod helpers;
mod mutations;
mod queries;
mod server;
mod subscriptions;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use async_graphql::{Context, Schema};
use crossbeam_channel::Sender;
use koan_core::audio::viz::VizSnapshot;
use koan_core::db::connection::Database;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueItemId, SharedPlayerState};
use uuid::Uuid;

use koan_core::auth::Role;
use mutations::MutationRoot;
use queries::QueryRoot;
pub use server::{
    ApiServerOpts, InProcessExecutor, cmd_serve, cmd_serve_daemon, decode_viz_frame,
    encode_viz_frame, execute_in_process, start_api_background,
};
use subscriptions::SubscriptionRoot;

use crate::auth::AuthUser;

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
// Schema builder
// ---------------------------------------------------------------------------

pub type KoanSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub fn build_schema(
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    db_path: PathBuf,
    viz: Option<Arc<VizSnapshot>>,
) -> KoanSchema {
    let mut builder = Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .data(DbHandle { path: db_path })
        .data(state)
        .data(cmd_tx);
    if let Some(viz) = viz {
        builder = builder.data(viz);
    }
    builder.finish()
}

// ---------------------------------------------------------------------------
// Shared helpers used by queries + mutations
// ---------------------------------------------------------------------------

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

        // Should send: ClearPlaylist, AddToPlaylist, Play
        let cmd1 = rx.try_recv().unwrap();
        assert!(
            matches!(cmd1, PlayerCommand::ClearPlaylist),
            "first command should be ClearPlaylist"
        );

        let cmd2 = rx.try_recv().unwrap();
        match cmd2 {
            PlayerCommand::AddToPlaylist(items) => {
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected AddToPlaylist, got {:?}", other),
        }

        let cmd3 = rx.try_recv().unwrap();
        assert!(
            matches!(cmd3, PlayerCommand::Play(_)),
            "third command should be Play"
        );
    }

    // ---- New Phase 1 tests: queue status, viz, config, playlist version ----

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

    // ---- Binary viz frame encoding/decoding roundtrip tests ----

    #[test]
    fn viz_frame_binary_roundtrip_no_waveform() {
        use koan_core::audio::viz::{NUM_BARS, VizFrame};

        let mut spectrum = [0.0f32; NUM_BARS];
        spectrum[0] = 0.99;
        spectrum[47] = 0.42;

        let mut peaks = [0.0f32; NUM_BARS];
        peaks[10] = 0.77;

        let frame = VizFrame {
            spectrum,
            peaks,
            vu_levels: [0.65, 0.31],
            beat_energy: 0.88,
            timestamp: std::time::Instant::now(),
            waveform: vec![0.1, -0.2, 0.3, -0.4],
        };

        let encoded = server::encode_viz_frame(&frame, false);

        // Header: 4 (magic) + 4 (bar count) + 48*4*2 (spectrum+peaks) + 8 (VU) + 4 (beat) + 4 (waveform len) = 408
        assert_eq!(encoded.len(), 4 + 4 + 48 * 4 * 2 + 8 + 4 + 4);

        let (spectrum_d, peaks_d, vu_d, beat_d, waveform_d) =
            server::decode_viz_frame(&encoded).expect("decode should succeed");

        assert_eq!(spectrum_d.len(), NUM_BARS);
        assert!((spectrum_d[0] - 0.99).abs() < 1e-6);
        assert!((spectrum_d[47] - 0.42).abs() < 1e-6);
        assert_eq!(peaks_d.len(), NUM_BARS);
        assert!((peaks_d[10] - 0.77).abs() < 1e-6);
        assert!((vu_d[0] - 0.65).abs() < 1e-6);
        assert!((vu_d[1] - 0.31).abs() < 1e-6);
        assert!((beat_d - 0.88).abs() < 1e-6);
        assert!(waveform_d.is_empty(), "waveform should be excluded");
    }

    #[test]
    fn viz_frame_binary_roundtrip_with_waveform() {
        use koan_core::audio::viz::{NUM_BARS, VizFrame};

        let waveform: Vec<f32> = (0..128).map(|i| (i as f32 / 128.0) * 2.0 - 1.0).collect();

        let frame = VizFrame {
            spectrum: [0.5; NUM_BARS],
            peaks: [0.3; NUM_BARS],
            vu_levels: [0.4, 0.6],
            beat_energy: 0.1,
            timestamp: std::time::Instant::now(),
            waveform: waveform.clone(),
        };

        let encoded = server::encode_viz_frame(&frame, true);
        let expected_len = 4 + 4 + 48 * 4 * 2 + 8 + 4 + 4 + 128 * 4;
        assert_eq!(encoded.len(), expected_len);

        let (spectrum_d, peaks_d, vu_d, beat_d, waveform_d) =
            server::decode_viz_frame(&encoded).expect("decode should succeed");

        assert_eq!(spectrum_d.len(), NUM_BARS);
        assert!(spectrum_d.iter().all(|&v| (v - 0.5).abs() < 1e-6));
        assert_eq!(peaks_d.len(), NUM_BARS);
        assert!(peaks_d.iter().all(|&v| (v - 0.3).abs() < 1e-6));
        assert!((vu_d[0] - 0.4).abs() < 1e-6);
        assert!((vu_d[1] - 0.6).abs() < 1e-6);
        assert!((beat_d - 0.1).abs() < 1e-6);
        assert_eq!(waveform_d.len(), 128);
        for (i, &v) in waveform_d.iter().enumerate() {
            assert!(
                (v - waveform[i]).abs() < 1e-6,
                "waveform mismatch at index {}",
                i
            );
        }
    }

    #[test]
    fn viz_frame_binary_decode_rejects_garbage() {
        assert!(server::decode_viz_frame(&[]).is_none());
        assert!(server::decode_viz_frame(&[0, 1, 2, 3]).is_none());
        assert!(server::decode_viz_frame(b"KVIZ").is_none());
        // Valid magic + bar_count=1 but truncated data.
        let mut bad = Vec::new();
        bad.extend_from_slice(b"KVIZ");
        bad.extend_from_slice(&1u32.to_le_bytes());
        assert!(server::decode_viz_frame(&bad).is_none());
    }
}

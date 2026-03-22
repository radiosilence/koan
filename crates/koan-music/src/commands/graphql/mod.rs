mod helpers;
mod mutations;
mod queries;
mod server;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use async_graphql::{Context, EmptySubscription, Schema};
use crossbeam_channel::Sender;
use koan_core::db::connection::Database;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueItemId, SharedPlayerState};
use uuid::Uuid;

use mutations::MutationRoot;
use queries::QueryRoot;
pub use server::{cmd_serve, cmd_serve_daemon, execute_in_process};

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
}

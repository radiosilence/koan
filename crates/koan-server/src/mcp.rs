//! MCP (Model Context Protocol) server for koan.
//!
//! Exposes the GraphQL schema as MCP tools for Claude Desktop / MCP clients.

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::SharedPlayerState;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Json;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, schemars, tool_router};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GraphqlParams {
    #[schemars(
        description = "GraphQL query or mutation string. Use the schema_sdl tool first to learn available types, queries, mutations, and filter parameters."
    )]
    pub query: String,
    #[schemars(description = "Optional JSON object of query variables")]
    pub variables: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// GraphQL execution result wrapper — MCP spec requires outputSchema to be an object type.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GraphqlResponse {
    /// The GraphQL response JSON (contains data and/or errors fields).
    pub result: serde_json::Value,
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct KoanMcpServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    graphql_schema: crate::graphql::KoanSchema,
}

impl KoanMcpServer {
    pub fn new(
        state: Arc<SharedPlayerState>,
        cmd_tx: Sender<PlayerCommand>,
        db_path: PathBuf,
    ) -> Self {
        let graphql_schema =
            crate::graphql::build_schema(state.clone(), cmd_tx.clone(), db_path.clone(), None);
        Self {
            tool_router: Self::tool_router(),
            graphql_schema,
        }
    }
}

use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;

#[tool_router]
impl KoanMcpServer {
    #[tool(
        description = "Get the full GraphQL schema in SDL format. CALL THIS FIRST to learn all \
        available queries, mutations, types, and filter parameters. The schema is the complete \
        reference for everything koan can do — library discovery, playback control, queue \
        management, favourites, snapshots, radio mode, device switching, and more."
    )]
    fn schema_sdl(&self) -> Json<GraphqlResponse> {
        let sdl = self.graphql_schema.sdl();
        Json(GraphqlResponse {
            result: serde_json::Value::String(sdl),
        })
    }

    #[tool(
        description = "Execute a GraphQL query or mutation against the koan music player. \
        This is the primary interface for ALL operations — library browsing, playback control, \
        queue management, favourites, snapshots, radio, devices.\n\n\
        Call schema_sdl first to learn the full schema.\n\n\
        Quick examples:\n\
        - Search: { tracks(search: \"aphex\") { edges { node { id title artist album } } } }\n\
        - Filter: { albums(yearEnd: 1995, codec: \"FLAC\") { edges { node { title artistName date } } } }\n\
        - Now playing: { nowPlaying { state positionMs track { title artist codec sampleRate } } }\n\
        - Queue tracks: mutation { addToQueue(trackIds: [42, 43]) { ok addedCount } }\n\
        - Play/pause: mutation { pause { ok } } / mutation { resume { ok } }\n\
        - Snapshot: mutation { saveSnapshot(name: \"techno\") { ok } }\n\
        - Radio: mutation { enableRadio { ok } }\n\n\
        Track IDs are integers from the library. Queue item IDs are UUIDs from the queue.\n\
        All string filters are case-insensitive substrings."
    )]
    fn graphql(
        &self,
        Parameters(params): Parameters<GraphqlParams>,
    ) -> Result<Json<GraphqlResponse>, String> {
        let schema = self.graphql_schema.clone();
        let query = params.query;
        let variables = params.variables;
        let rt =
            tokio::runtime::Handle::try_current().map_err(|_| "no tokio runtime".to_string())?;
        let result = tokio::task::block_in_place(|| {
            rt.block_on(crate::graphql::execute_in_process(
                &schema, &query, variables,
            ))
        });
        Ok(Json(GraphqlResponse { result }))
    }
}

#[rmcp::tool_handler]
impl ServerHandler for KoanMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "koan is a bit-perfect macOS music player. You control it entirely via GraphQL.\n\n\
             ## How to use\n\
             1. Call `schema_sdl` to get the full GraphQL schema\n\
             2. Use the `graphql` tool for ALL queries and mutations\n\n\
             ## What you can do\n\
             - **Discover music**: query `artists`, `albums`, `tracks` with rich filters \
               (genre, year range, codec, sample rate, bit depth, duration, favourites)\n\
             - **Control playback**: mutations `play`, `pause`, `resume`, `stop`, `next`, \
               `previous`, `seek`\n\
             - **Manage queue**: `addToQueue`, `replaceQueue`, `removeFromQueue`, `moveInQueue`, \
               `clearQueue`, `undo`, `redo`\n\
             - **Favourites**: `favourite`, `unfavourite`, `toggleFavourite` (auto-syncs to \
               Subsonic/Navidrome). Filter any query with `favouritesOnly: true`\n\
             - **Snapshots**: `saveSnapshot`, `restoreSnapshot`, `deleteSnapshot` — bank curated \
               mixes and switch between them\n\
             - **Radio**: `enableRadio`, `disableRadio` — auto-queues similar tracks\n\
             - **Devices**: query `devices`, mutation `setDevice`, `clearDevice`\n\
             - **History**: query `playHistory`, `similarArtists`\n\n\
             ## ID conventions\n\
             - Track IDs: integers from the library database\n\
             - Queue item IDs: UUIDs assigned when tracks enter the queue",
        )
    }
}

/// Entry point for `koan mcp` — starts a headless player with an MCP server on stdio.
pub fn cmd_mcp() {
    use koan_core::player::Player;
    use rmcp::ServiceExt;

    // Validate DB is accessible before starting the server.
    let _db = koan_core::db::connection::Database::open_default().expect("failed to open database");
    let db_path = koan_core::config::db_path();

    // Spawn the player engine (headless — no TUI).
    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    let server = KoanMcpServer::new(state, cmd_tx, db_path);

    // Run the MCP server on the tokio runtime (blocking the main thread).
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let transport = rmcp::transport::io::stdio();
        let service = server
            .serve(transport)
            .await
            .expect("failed to start MCP server");
        let _ = service.waiting().await;
    });
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

    fn test_server() -> (KoanMcpServer, CommandChannel, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();

        let state = SharedPlayerState::new();
        let ch = CommandChannel::new();
        let tx = ch.tx.clone();

        let server = KoanMcpServer::new(state, tx, db_path);
        (server, ch, tmp)
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
    fn schema_sdl_returns_schema() {
        let (server, _ch, _tmp) = test_server();
        let Json(resp) = server.schema_sdl();
        let sdl = resp.result.as_str().unwrap();
        assert!(sdl.contains("type QueryRoot"));
        assert!(sdl.contains("type MutationRoot"));
        assert!(sdl.contains("artists"));
        assert!(sdl.contains("nowPlaying"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graphql_query_works() {
        let (server, _ch, tmp) = test_server();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "Windowlicker", "Aphex Twin", "Windowlicker EP");

        let result = server.graphql(Parameters(GraphqlParams {
            query: r#"{ tracks(search: "aphex") { edges { node { title artist } } } }"#.into(),
            variables: None,
        }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        let data = &resp.result["data"]["tracks"]["edges"];
        assert_eq!(data.as_array().unwrap().len(), 1);
        assert_eq!(data[0]["node"]["title"], "Windowlicker");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graphql_mutation_works() {
        let (server, _ch, _tmp) = test_server();
        let result = server.graphql(Parameters(GraphqlParams {
            query: "mutation { pause { ok message } }".into(),
            variables: None,
        }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.result["data"]["pause"]["ok"], true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graphql_now_playing_stopped() {
        let (server, _ch, _tmp) = test_server();
        let result = server.graphql(Parameters(GraphqlParams {
            query: "{ nowPlaying { state positionMs } }".into(),
            variables: None,
        }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.result["data"]["nowPlaying"]["state"], "STOPPED");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graphql_library_stats() {
        let (server, _ch, tmp) = test_server();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "T1", "A1", "Album1");

        let result = server.graphql(Parameters(GraphqlParams {
            query: "{ libraryStats { totalTracks totalArtists totalAlbums } }".into(),
            variables: None,
        }));
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert_eq!(resp.result["data"]["libraryStats"]["totalTracks"], 1);
    }
}

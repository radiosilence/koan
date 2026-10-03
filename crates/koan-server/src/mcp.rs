//! MCP (Model Context Protocol) server for koan.
//!
//! Exposes the GraphQL schema as MCP tools for Claude Desktop / MCP clients.

use std::sync::Arc;

use crate::auth::AuthUser;
use crossbeam_channel::Sender;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::SharedPlayerState;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Json;
use rmcp::model::{ServerCapabilities, ServerConfig};
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
    /// Served over HTTP: a server whose music plays on the devices linked to it.
    http: bool,
}

impl KoanMcpServer {
    pub fn new(
        state: Arc<SharedPlayerState>,
        cmd_tx: Sender<PlayerCommand>,
        pool: Arc<koan_core::db::pool::Pool>,
    ) -> Self {
        let graphql_schema = crate::graphql::build_schema(state, cmd_tx, pool, None);
        Self {
            tool_router: Self::tool_router(),
            graphql_schema,
            http: false,
        }
    }

    /// Who a request acts as: the account `bearer_gate` found its token for,
    /// at most at `capped` role; on stdio, the local user at `mcp_role()`.
    fn caller(&self, extensions: &rmcp::model::Extensions) -> AuthUser {
        extensions
            .get::<axum::http::request::Parts>()
            .and_then(|p| p.extensions.get::<AuthUser>())
            .map(|u| AuthUser {
                role: capped(u.role),
                ..u.clone()
            })
            .unwrap_or_else(|| AuthUser {
                user_id: koan_core::db::queries::LOCAL_USER,
                role: mcp_role(),
                ..AuthUser::anonymous_admin()
            })
    }
}

use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;

/// Role the MCP `graphql` tool executes at.
///
/// The transport carries no credential, so anything reachable here is reachable
/// by whoever can talk to the MCP process. `User` covers everything the tool
/// advertises — browsing, playback, queue, favourites, playlists, radio — and
/// leaves out the admin mutations that move files on disk (`organize*`), rewrite
/// config, or change the output device. `KOAN_MCP_ADMIN=1` opts back in.
fn mcp_role() -> koan_core::auth::Role {
    if std::env::var("KOAN_MCP_ADMIN").is_ok_and(|v| v == "1") {
        koan_core::auth::Role::Admin
    } else {
        koan_core::auth::Role::User
    }
}

/// The role an account acts at through the MCP: its own, but no higher than
/// `mcp_role()`. Track titles, tags and share descriptions reach the model, and
/// any of them may carry an instruction; capped, an admin account's model
/// cannot be talked into moving files or rewriting config.
pub fn capped(role: koan_core::auth::Role) -> koan_core::auth::Role {
    use koan_core::auth::Role;
    match (role, mcp_role()) {
        (Role::Admin, Role::User) => Role::User,
        (role, _) => role,
    }
}

#[tool_router]
impl KoanMcpServer {
    #[tool(
        description = "The GraphQL schema for the user's music (kōan): their library and the \
        players they listen on. Call this first, before `graphql`. It covers playing, pausing, \
        skipping and queueing music on the user's phone and computers, what is playing now, \
        and searching, browsing and making playlists from the music they own."
    )]
    fn schema_sdl(&self) -> Json<GraphqlResponse> {
        let sdl = self.graphql_schema.sdl();
        Json(GraphqlResponse {
            result: serde_json::Value::String(sdl),
        })
    }

    #[tool(
        description = "Control the user's music and search their music library (kōan). Use it \
        for any request about music they listen to or own: play something, pause, resume, skip, \
        what's playing, what's next, add to or change the queue, find or recommend from their \
        collection, playlists, favourites. \"Pause the music on my desktop\", \"play some \
        jazz on my phone\" and \"what is this song\" are all this tool.\n\n\
        Call schema_sdl first for the full schema. The user's phones and computers running \
        kōan are `clients`; commands for them end in `OnClient`.\n\n\
        Examples:\n\
        - What's playing, where: { clients { name playing nowPlaying positionMs } }\n\
        - Pause: mutation { controlClient(action: PAUSE) { ok message } }\n\
        - Find music: { tracks(search: \"aphex\", first: 20) { edges { node { id title artist album } } } }\n\
        - Play it: mutation { playOnClient(trackIds: [\"42\", \"43\"]) { ok message } }\n\n\
        String filters are case-insensitive substrings."
    )]
    async fn graphql(
        &self,
        Parameters(params): Parameters<GraphqlParams>,
        extensions: rmcp::model::Extensions,
    ) -> Json<GraphqlResponse> {
        let caller = self.caller(&extensions);
        let result = crate::graphql::execute_in_process(
            &self.graphql_schema,
            &params.query,
            params.variables,
            caller,
        )
        .await;
        Json(GraphqlResponse { result })
    }
}

#[rmcp::tool_handler]
impl ServerHandler for KoanMcpServer {
    fn get_info(&self) -> ServerConfig {
        // Over HTTP this is a server: its own player is headless and nobody
        // hears it, and what the user listens to is the apps linked to it. On
        // stdio it is the user's own machine, and its player is the music.
        let instructions = if self.http {
            SERVER_INSTRUCTIONS
        } else {
            LOCAL_INSTRUCTIONS
        };
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "koan",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(instructions)
    }
}

const SERVER_INSTRUCTIONS: &str = "kōan is the user's music: their whole music library, and the \
phones and computers they listen on. Use it for anything about music they are playing or own — \
\"pause the music\", \"play something like Polar Bear on my phone\", \"what's this song\", \
\"skip to the Phace remix\", \"add their new album when it's downloaded\". Call `schema_sdl` \
once, then do everything through `graphql`.

## Where the music plays
The user listens in kōan apps on their devices, linked to this server. Query \
`clients { name platform playing nowPlaying album positionMs durationMs radio queue { trackId \
title artist current } }` to see each device, what it is playing and what it has queued. Every \
command about the user's music goes to a device:
- `controlClient(action: PAUSE|RESUME|NEXT|PREVIOUS)`, `seekOnClient(positionMs)`
- `playOnClient(trackIds, startAt)` replaces the queue and plays; `enqueue: true` appends. \
A phone iOS has suspended is not linked but is still reached. Music comes up there as a \
notification to tap, since iOS lets no app start audio on its own from sleep; queue, radio and \
other changes are applied as it wakes. The message says when a device was asleep: tell the user \
to tap the notification
- `playNextOnClient(trackIds)`, `jumpOnClient(trackId)` (skip to a track, queued or not), \
`removeFromClient(trackIds)`, `clearClient`, `setClientRadio(enabled)`, `syncClient`
- **Making a playlist the user asked for** (\"make me a cyberpunk playlist\"): research what \
fits, find each track in the library, `createPlaylist` with those in order. For picks the \
library lacks, fetch the album with slsk's `grab`, then `addToPlaylistWhenAdded(playlistId, \
artist, album, titles)` to add the wanted tracks once it is imported. Tell the user what is \
there now and what is on its way.
- Playlists made or edited here (`createPlaylist`, `setPlaylistTracks`…) reach every device \
by themselves: linked ones sync at once, others when next opened. `syncClients` does the same \
on request.
- `evictOnClients(trackIds)` makes every linked device drop its downloaded copies of those \
tracks: when a track plays as noise or glitches, after the file on the server is replaced
- `queueOnClientWhenAdded(artist, album)` queues an album once it reaches the library, e.g. \
one being downloaded with slsk's `grab`; `clientOrders` lists those waiting
Leave `client` out unless the user named a device (\"my phone\", \"the desktop\": match it \
against `clients` names and platforms). Without it the server picks the device that is \
playing, else the one played most recently; if it answers that it cannot tell, ask the user \
which device.

**Act on what the user asks; do not second-guess it from reported state.** \"Pause\", \
\"skip\" and \"resume\" go straight to `controlClient`: the user can hear the device and you \
cannot, and a report can be stale or, from an older app (`playing: null`), absent.

**Never use the server's own player for the user's music.** `play`, `pause`, `resume`, \
`next`, `previous`, `seek`, `nowPlaying`, `queue`, `addToQueue`, `replaceQueue`, \
`playPlaylist` and the radio mutations drive a headless player on the server that nobody \
hears; `nowPlaying` there reports nothing about what the user is listening to.

## The library
- `artists`, `albums`, `tracks` with filters (genre, year range, codec, sample rate, bit depth, \
duration, favourites), `randomTracks`, `similarArtists`, `similarTracks`, `fuzzySearch`
- Build a set from these, then send its track ids to a device with `playOnClient`. Track ids are \
integers in queries; pass them to the client mutations as strings.
- Favourites: `favourite`, `unfavourite`, `toggleFavourite`, `favouritesOnly: true` on queries
- Playlists: `playlists`, `playlistTracks`, `createPlaylist`, `addToPlaylist`, \
`setPlaylistTracks`, `renamePlaylist`, `deletePlaylist`
- History: `playHistory`
- Sharing: `createShare(trackIds, description)` makes a public link anyone can open without an \
account; confirm with the user first. `shares`, `updateShare`, `deleteShare` manage them.

## Not available
`organize*` (moves files on disk), `updateConfig` and `triggerScan` are refused unless \
`KOAN_MCP_ADMIN=1` is set.";

const LOCAL_INSTRUCTIONS: &str = "kōan is the user's music player on this machine and their \
music library. Use it for anything about music they are playing or own — \"pause the music\", \
\"play something like Polar Bear\", \"what's this song\". Call `schema_sdl` once, then do \
everything through `graphql`.

## Playback
This player is what the user hears: `play`, `pause`, `resume`, `stop`, `next`, `previous`, \
`seek`, `nowPlaying`; the queue with `queue`, `addToQueue`, `replaceQueue`, `removeFromQueue`, \
`moveInQueue`, `clearQueue`, `undo`, `redo`; radio with `enableRadio`, `disableRadio`.

## The library
- `artists`, `albums`, `tracks` with filters (genre, year range, codec, sample rate, bit depth, \
duration, favourites), `randomTracks`, `similarArtists`, `similarTracks`, `fuzzySearch`
- Favourites: `favourite`, `unfavourite`, `toggleFavourite`, `favouritesOnly: true` on queries
- Playlists: `playlists`, `playlistTracks`, `createPlaylist`, `saveQueueAsPlaylist`, \
`addToPlaylist`, `setPlaylistTracks`, `renamePlaylist`, `deletePlaylist`, `playPlaylist`
- History: `playHistory`
- Sharing: `createShare(trackIds, description)` makes a public link; confirm with the user first.

## Not available
`organize*` (moves files on disk), `updateConfig`, `triggerScan` and `setDevice` are refused \
unless `KOAN_MCP_ADMIN=1` is set.

## IDs
Track IDs are integers from the library; queue item IDs are UUIDs from the queue.";

const MAX_BODY: usize = 1024 * 1024;
/// Open event streams count against it, so it allows a few clients each with
/// a stream and requests in flight.
const MAX_CONCURRENT: usize = 64;

/// `/mcp` on the main port, for clients holding a token from koan's own OAuth
/// (`ui::oauth`). Each request acts as the account its token names, at that
/// account's role.
pub fn router(
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    auth: crate::auth::middleware::AuthState,
    public_url: Option<String>,
) -> axum::Router {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let mut template = KoanMcpServer::new(state, cmd_tx, auth.pool.clone());
    template.http = true;
    let service = StreamableHttpService::new(
        move || Ok(template.clone()),
        Arc::new(LocalSessionManager::default()),
        // The main app's Host guard has already checked the Host.
        StreamableHttpServerConfig::default().disable_allowed_hosts(),
    );
    // No request timeout: a session's GET is an event stream that stays open.
    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(MAX_BODY))
        .layer(axum::middleware::from_fn_with_state(
            (auth, public_url),
            bearer_gate,
        ))
        .layer(
            tower::ServiceBuilder::new()
                .layer(axum::error_handling::HandleErrorLayer::new(
                    |_: tower::BoxError| async {
                        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "busy")
                    },
                ))
                .load_shed()
                .concurrency_limit(MAX_CONCURRENT),
        )
}

/// Let a request with a valid access token through as its account. Without
/// one, the 401 names the resource metadata, which is how a client finds where
/// to sign in.
async fn bearer_gate(
    axum::extract::State((auth, public_url)): axum::extract::State<(
        crate::auth::middleware::AuthState,
        Option<String>,
    )>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    let user = if auth.auth_enabled {
        let token = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .and_then(|t| koan_core::auth::validate_access_token(&auth.public_pem, t).ok());
        match token {
            Some(claims) => crate::auth::current_user(&auth.pool, claims).await,
            None => None,
        }
    } else {
        Some(AuthUser::anonymous_admin())
    };
    match user {
        Some(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        None => {
            // Without `public_url` there is no OAuth to point the client at.
            let challenge = match public_url.as_deref().map(|u| u.trim_end_matches('/')) {
                Some(base) => format!(
                    "Bearer resource_metadata=\"{base}{}\"",
                    crate::ui::RESOURCE_METADATA
                ),
                None => "Bearer".to_owned(),
            };
            (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, challenge)],
                "sign in to kōan",
            )
                .into_response()
        }
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

    let pool = Arc::new(koan_core::db::pool::Pool::new(db_path));
    let server = KoanMcpServer::new(state, cmd_tx, pool);

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

        let server =
            KoanMcpServer::new(state, tx, Arc::new(koan_core::db::pool::Pool::new(db_path)));
        (server, ch, tmp)
    }

    fn as_user(user: AuthUser) -> rmcp::model::Extensions {
        let (mut parts, ()) = axum::http::Request::new(()).into_parts();
        parts.extensions.insert(user);
        let mut ext = rmcp::model::Extensions::new();
        ext.insert(parts);
        ext
    }

    #[test]
    fn a_token_acts_as_its_account_with_admin_capped() {
        use koan_core::auth::Role;
        let (server, _ch, _tmp) = test_server();
        let user = |role| AuthUser {
            user_id: 7,
            username: "mate".into(),
            role,
        };
        let c = server.caller(&as_user(user(Role::Admin)));
        assert_eq!((c.user_id, c.username.as_str()), (7, "mate"));
        assert_eq!(c.role, capped(Role::Admin));
        assert_eq!(
            server.caller(&as_user(user(Role::Readonly))).role,
            Role::Readonly
        );
        // stdio: the local user, at the transport's default role.
        let local = server.caller(&Default::default());
        assert_eq!(
            (local.user_id, local.role),
            (queries::LOCAL_USER, mcp_role())
        );
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
            album_mbid: None,
            album_added_at: None,
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

        let result = server
            .graphql(
                Parameters(GraphqlParams {
                    query: r#"{ tracks(search: "aphex") { edges { node { title artist } } } }"#
                        .into(),
                    variables: None,
                }),
                Default::default(),
            )
            .await;
        let Json(resp) = result;
        let data = &resp.result["data"]["tracks"]["edges"];
        assert_eq!(data.as_array().unwrap().len(), 1);
        assert_eq!(data[0]["node"]["title"], "Windowlicker");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graphql_mutation_works() {
        let (server, _ch, _tmp) = test_server();
        let result = server
            .graphql(
                Parameters(GraphqlParams {
                    query: "mutation { pause { ok message } }".into(),
                    variables: None,
                }),
                Default::default(),
            )
            .await;
        let Json(resp) = result;
        assert_eq!(resp.result["data"]["pause"]["ok"], true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graphql_now_playing_stopped() {
        let (server, _ch, _tmp) = test_server();
        let result = server
            .graphql(
                Parameters(GraphqlParams {
                    query: "{ nowPlaying { state positionMs } }".into(),
                    variables: None,
                }),
                Default::default(),
            )
            .await;
        let Json(resp) = result;
        assert_eq!(resp.result["data"]["nowPlaying"]["state"], "STOPPED");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graphql_library_stats() {
        let (server, _ch, tmp) = test_server();
        let db_path = tmp.path().join("test.db");
        insert_test_track(&db_path, "T1", "A1", "Album1");

        let result = server
            .graphql(
                Parameters(GraphqlParams {
                    query: "{ libraryStats { totalTracks totalArtists totalAlbums } }".into(),
                    variables: None,
                }),
                Default::default(),
            )
            .await;
        let Json(resp) = result;
        assert_eq!(resp.result["data"]["libraryStats"]["totalTracks"], 1);
    }
}

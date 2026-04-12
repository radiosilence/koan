use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::audio::viz::VizSnapshot;
use koan_core::auth::{self, parse_duration_secs};
use koan_core::config::Config;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::SharedPlayerState;

use super::{KoanSchema, build_schema};
use crate::auth::AuthUser;
use crate::auth::middleware::{AuthState, auth_middleware};
use crate::auth::routes::{AuthRouteState, auth_router};

// ---------------------------------------------------------------------------
// `koan --headless` entry point (standalone headless server)
// ---------------------------------------------------------------------------

pub fn cmd_serve(
    port: Option<u16>,
    bind: Option<std::net::IpAddr>,
    subsonic_port: Option<u16>,
    playground: bool,
) {
    use koan_core::player::Player;

    // Validate DB is accessible before starting the server.
    let _db = koan_core::db::connection::Database::open_default().expect("failed to open database");
    let db_path = koan_core::config::db_path();

    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    run_api_blocking(ApiServerOpts {
        state,
        cmd_tx,
        db_path,
        port,
        bind,
        subsonic_port,
        playground,
        viz: None, // headless — no viz analyzer
    });
}

// ---------------------------------------------------------------------------
// Shared API server logic — used by both headless and TUI+API modes
// ---------------------------------------------------------------------------

/// Options for the API server — avoids too-many-arguments.
pub struct ApiServerOpts {
    pub state: Arc<SharedPlayerState>,
    pub cmd_tx: Sender<PlayerCommand>,
    pub db_path: PathBuf,
    pub port: Option<u16>,
    pub bind: Option<std::net::IpAddr>,
    pub subsonic_port: Option<u16>,
    pub playground: bool,
    pub viz: Option<Arc<VizSnapshot>>,
}

/// Run the GraphQL (+ optional Subsonic) API server, blocking the current thread.
/// Called from `cmd_serve` (headless) and `start_api_background` (TUI companion).
fn run_api_blocking(opts: ApiServerOpts) {
    let ApiServerOpts {
        state,
        cmd_tx,
        db_path,
        port,
        bind,
        subsonic_port,
        playground,
        viz,
    } = opts;
    use axum::routing::{get, post};

    let cfg = Config::load().unwrap_or_default();
    let port = port.unwrap_or(cfg.graphql.port);
    let bind = bind.unwrap_or(cfg.graphql.bind);
    let playground_enabled = playground || cfg.graphql.playground;
    let auth_enabled = cfg.graphql.auth_enabled;

    // Load or generate Ed25519 keypair for JWT signing.
    let (private_pem, public_pem) = if auth_enabled {
        match auth::load_keypair() {
            Ok(kp) => kp,
            Err(e) => {
                panic!(
                    "Auth enabled but keypair not found: {}. Run `koan auth setup` first. \
                     Refusing to start with auth_enabled=true and no valid keypair.",
                    e
                );
            }
        }
    } else {
        // When auth is disabled, we still need dummy keys for the route state
        // (routes exist but won't be hit by middleware). Generate if available.
        auth::load_or_generate_keypair().unwrap_or_default()
    };

    let auth_actually_enabled = auth_enabled && !private_pem.is_empty();

    let access_ttl = parse_duration_secs(&cfg.graphql.access_token_ttl).unwrap_or(900);
    let refresh_ttl = parse_duration_secs(&cfg.graphql.refresh_token_ttl).unwrap_or(2_592_000);

    // Generate a process-scoped introspection key for playground access.
    let introspection_key = if playground_enabled && auth_actually_enabled {
        // Use UUID v4 as a random introspection key — 122 bits of randomness, no extra deps.
        Some(Arc::new(uuid::Uuid::now_v7().to_string()))
    } else {
        None
    };

    let auth_state = AuthState {
        public_pem: Arc::new(public_pem.clone()),
        auth_enabled: auth_actually_enabled,
        introspection_key: introspection_key.clone(),
    };

    let auth_route_state = AuthRouteState {
        db_path: db_path.clone(),
        private_pem: Arc::new(private_pem),
        public_pem: Arc::new(public_pem),
        access_ttl_secs: access_ttl,
        refresh_ttl_secs: refresh_ttl,
    };

    let schema = build_schema(state, cmd_tx, db_path.clone(), viz);

    if auth_actually_enabled {
        log::info!(
            "Auth enabled (Ed25519 JWT, access TTL {}s, refresh TTL {}s)",
            access_ttl,
            refresh_ttl
        );
    } else {
        log::info!("Auth disabled — all requests treated as admin");
    }

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        // GraphQL routes — protected by auth middleware.
        let gql_app = axum::Router::new()
            .route("/graphql", post(graphql_handler))
            .route("/graphql/ws", get(graphql_ws_handler))
            .layer(axum::middleware::from_fn_with_state(
                auth_state.clone(),
                auth_middleware,
            ))
            .layer(tower::limit::ConcurrencyLimitLayer::new(10))
            .with_state(schema);

        // Auth routes — always accessible (no auth middleware).
        let auth_app = auth_router(auth_route_state);

        // CORS — allow cross-origin requests for browser clients.
        let cors = tower_http::cors::CorsLayer::new()
            .allow_origin(tower_http::cors::Any)
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers(tower_http::cors::Any);

        let mut app = auth_app.merge(gql_app).layer(cors);
        if playground_enabled {
            app = app.route(
                "/graphql",
                get(graphql_playground).with_state(introspection_key.clone()),
            );
        }

        // Build playground URL with introspection key.
        let playground_url = if playground_enabled {
            if let Some(ref key) = introspection_key {
                format!("http://{}:{}/graphql?introspection-key={}", bind, port, key)
            } else {
                format!("http://{}:{}/graphql", bind, port)
            }
        } else {
            format!("http://{}:{}/graphql", bind, port)
        };

        let gql_addr = std::net::SocketAddr::new(bind, port);

        let gql_listener = match tokio::net::TcpListener::bind(gql_addr).await {
            Ok(l) => {
                log::info!("GraphQL API on http://{}:{}/graphql", bind, port);
                if playground_enabled {
                    log::info!("GraphiQL: {}", playground_url);
                    // Open browser on macOS/Linux.
                    #[cfg(target_os = "macos")]
                    let _ = std::process::Command::new("open").arg(&playground_url).spawn();
                    #[cfg(target_os = "linux")]
                    let _ = std::process::Command::new("xdg-open").arg(&playground_url).spawn();
                }
                l
            }
            Err(e) => {
                log::warn!(
                    "API disabled: failed to bind GraphQL port {} — {} (another instance running?)",
                    port,
                    e,
                );
                return;
            }
        };
        let gql_server =
            axum::serve(gql_listener, app).with_graceful_shutdown(shutdown_signal());

        if let Some(sub_port) = subsonic_port {
            if let Some(sub_app) = crate::subsonic::subsonic_router(db_path) {
                let sub_addr = std::net::SocketAddr::new(bind, sub_port);

                match tokio::net::TcpListener::bind(sub_addr).await {
                    Ok(sub_listener) => {
                        log::info!("Subsonic REST on http://{}:{}/rest/", bind, sub_port);
                        let sub_server = axum::serve(sub_listener, sub_app)
                            .with_graceful_shutdown(shutdown_signal());

                        tokio::select! {
                            r = gql_server => { if let Err(e) = r { log::error!("GraphQL server error: {e}"); } },
                            r = sub_server => { if let Err(e) = r { log::error!("Subsonic server error: {e}"); } },
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "Subsonic API disabled: failed to bind port {} — {}",
                            sub_port,
                            e,
                        );
                        // Run GraphQL-only.
                        if let Err(e) = gql_server.await {
                            log::error!("GraphQL server error: {e}");
                        }
                    }
                }
            } else {
                // subsonic_router returned None — credentials not configured.
                if let Err(e) = gql_server.await {
                    log::error!("GraphQL server error: {e}");
                }
            }
        } else {
            if let Err(e) = gql_server.await {
                log::error!("GraphQL server error: {e}");
            }
        }
    });
}

/// Start the API server on the current thread (blocks forever).
/// Called from a background thread when TUI mode has API enabled.
///
/// Accepts positional args for backward compatibility with koan-cli.
/// Prefer `ApiServerOpts` for new call sites.
pub fn start_api_background(
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    db_path: PathBuf,
    port: Option<u16>,
    bind: Option<std::net::IpAddr>,
    subsonic_port: Option<u16>,
    playground: bool,
) {
    run_api_blocking(ApiServerOpts {
        state,
        cmd_tx,
        db_path,
        port,
        bind,
        subsonic_port,
        playground,
        viz: None,
    });
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
}

async fn graphql_handler(
    axum::Extension(user): axum::Extension<AuthUser>,
    axum::extract::State(schema): axum::extract::State<KoanSchema>,
    req: async_graphql_axum::GraphQLRequest,
) -> async_graphql_axum::GraphQLResponse {
    let mut request = req.into_inner();
    // The auth middleware always injects AuthUser (anonymous_admin when auth is
    // disabled, or a real user when auth is enabled). No fallback needed here.
    request = request.data(user);
    schema.execute(request).await.into()
}

async fn graphql_ws_handler(
    axum::Extension(user): axum::Extension<AuthUser>,
    axum::extract::State(schema): axum::extract::State<KoanSchema>,
    protocol: async_graphql_axum::GraphQLProtocol,
    websocket: axum::extract::WebSocketUpgrade,
) -> axum::response::Response {
    websocket
        .protocols(async_graphql::http::ALL_WEBSOCKET_PROTOCOLS)
        .on_upgrade(move |stream| {
            let stream = async_graphql_axum::GraphQLWebSocket::new(stream, schema, protocol)
                .on_connection_init(move |_| async move {
                    let mut data = async_graphql::Data::default();
                    data.insert(user);
                    Ok(data)
                });
            async move {
                stream.serve().await;
            }
        })
}

async fn graphql_playground(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    axum::extract::State(key): axum::extract::State<Option<Arc<String>>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    // If an introspection key exists, require it in the URL.
    if let Some(ref expected) = key {
        let provided = params.get("introspection-key");
        if provided.map(|k| k.as_str()) != Some(expected.as_str()) {
            return (
                axum::http::StatusCode::FORBIDDEN,
                "invalid or missing introspection-key",
            )
                .into_response();
        }
    }

    // Use async-graphql's built-in GraphiQL (self-contained, no CDN).
    // Inject the introspection key as a default header so all queries are authed.
    let mut source = async_graphql::http::GraphiQLSource::build().endpoint("/graphql");
    if let Some(ref k) = key {
        source = source.header("X-Introspection-Key", k.as_str());
    }

    axum::response::Html(source.finish()).into_response()
}

/// Run the server as a background daemon (fork + detach).
pub fn cmd_serve_daemon(
    port: Option<u16>,
    bind: Option<std::net::IpAddr>,
    subsonic_port: Option<u16>,
    playground: bool,
) {
    use std::fs;
    use std::process::Command;

    let cfg = Config::load().unwrap_or_default();
    let port_val = port.unwrap_or(cfg.graphql.port);
    let bind_val = bind.unwrap_or(cfg.graphql.bind);

    let exe = std::env::current_exe().expect("failed to get current exe path");
    let mut cmd = Command::new(exe);
    // Use the new unified CLI: `koan --headless --port <port>`
    cmd.arg("--headless");
    cmd.arg("--port").arg(port_val.to_string());
    cmd.arg("--bind").arg(bind_val.to_string());
    if let Some(sp) = subsonic_port {
        cmd.arg("--subsonic").arg(sp.to_string());
    }
    if playground || cfg.graphql.playground {
        cmd.arg("--playground");
    }

    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd.spawn().expect("failed to spawn daemon process");
    let pid = child.id();

    let pid_path = koan_core::config::config_dir().join("koan-serve.pid");
    fs::write(&pid_path, pid.to_string()).ok();

    std::thread::spawn(move || {
        let _ = child.wait();
    });

    eprintln!("koan daemon started (pid {}) on port {}", pid, port_val);
    if let Some(sp) = subsonic_port {
        eprintln!("  Subsonic REST on port {}", sp);
    }
    eprintln!("  PID file: {}", pid_path.display());
}

// ---------------------------------------------------------------------------
// In-process execution (for MCP `graphql` tool)
// ---------------------------------------------------------------------------

/// Execute a GraphQL query in-process (no HTTP round-trip).
/// In-process requests bypass auth — they're trusted (e.g., MCP tools).
pub async fn execute_in_process(
    schema: &KoanSchema,
    query: &str,
    variables: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut request = async_graphql::Request::new(query);
    // In-process = admin access (MCP/local tools).
    request = request.data(AuthUser::anonymous_admin());
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

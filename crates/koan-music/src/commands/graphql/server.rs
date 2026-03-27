use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::config::Config;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::SharedPlayerState;

use super::{KoanSchema, build_schema};

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
    let _db = super::super::open_db();
    let db_path = koan_core::config::db_path();

    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    run_api_blocking(
        state,
        cmd_tx,
        db_path,
        port,
        bind,
        subsonic_port,
        playground,
    );
}

// ---------------------------------------------------------------------------
// Shared API server logic — used by both headless and TUI+API modes
// ---------------------------------------------------------------------------

/// Run the GraphQL (+ optional Subsonic) API server, blocking the current thread.
/// Called from `cmd_serve` (headless) and `start_api_background` (TUI companion).
fn run_api_blocking(
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    db_path: PathBuf,
    port: Option<u16>,
    bind: Option<std::net::IpAddr>,
    subsonic_port: Option<u16>,
    playground: bool,
) {
    use axum::routing::{get, post};

    let cfg = Config::load().unwrap_or_default();
    let port = port.unwrap_or(cfg.graphql.port);
    let bind = bind.unwrap_or(cfg.graphql.bind);
    let playground_enabled = playground || cfg.graphql.playground;

    let schema = build_schema(state, cmd_tx, db_path.clone());

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let mut gql_app = axum::Router::new().route("/graphql", post(graphql_handler));
        if playground_enabled {
            gql_app = gql_app.route("/graphql", get(graphql_playground));
        }
        let gql_app = gql_app.with_state(schema);

        let gql_addr = std::net::SocketAddr::new(bind, port);

        let gql_listener = match tokio::net::TcpListener::bind(gql_addr).await {
            Ok(l) => {
                log::info!("GraphQL API on http://{}:{}/graphql", bind, port);
                if playground_enabled {
                    log::info!("GraphiQL: http://{}:{}/graphql", bind, port);
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
            axum::serve(gql_listener, gql_app).with_graceful_shutdown(shutdown_signal());

        if let Some(sub_port) = subsonic_port {
            let sub_app = super::super::serve::subsonic_router(db_path);
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
            if let Err(e) = gql_server.await {
                log::error!("GraphQL server error: {e}");
            }
        }
    });
}

/// Start the API server on the current thread (blocks forever).
/// Called from a background thread when TUI mode has API enabled.
pub fn start_api_background(
    state: Arc<SharedPlayerState>,
    cmd_tx: Sender<PlayerCommand>,
    db_path: PathBuf,
    port: Option<u16>,
    bind: Option<std::net::IpAddr>,
    subsonic_port: Option<u16>,
    playground: bool,
) {
    run_api_blocking(
        state,
        cmd_tx,
        db_path,
        port,
        bind,
        subsonic_port,
        playground,
    );
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
}

async fn graphql_handler(
    axum::extract::State(schema): axum::extract::State<KoanSchema>,
    req: async_graphql_axum::GraphQLRequest,
) -> async_graphql_axum::GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

async fn graphql_playground() -> axum::response::Html<String> {
    axum::response::Html(
        async_graphql::http::GraphiQLSource::build()
            .endpoint("/graphql")
            .finish(),
    )
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

use koan_core::config::Config;

use super::{KoanSchema, build_schema};

// ---------------------------------------------------------------------------
// `koan graphql` entry point
// ---------------------------------------------------------------------------

pub fn cmd_serve(port: Option<u16>, subsonic_port: Option<u16>, playground: bool) {
    use axum::routing::{get, post};
    use koan_core::player::Player;

    let _db = super::super::open_db();
    let db_path = koan_core::config::db_path();

    let cfg = Config::load().unwrap_or_default();
    let port = port.unwrap_or(cfg.graphql.port);
    let playground_enabled = playground || cfg.graphql.playground;

    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    let schema = build_schema(state, cmd_tx, db_path.clone());

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let mut gql_app = axum::Router::new().route("/graphql", post(graphql_handler));
        if playground_enabled {
            gql_app = gql_app.route("/graphql", get(graphql_playground));
        }
        let gql_app = gql_app.with_state(schema);

        let gql_addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        eprintln!("koan serve — GraphQL on http://0.0.0.0:{}/graphql", port);
        if playground_enabled {
            eprintln!("  Playground: http://localhost:{}/graphql", port);
        }

        let gql_listener = tokio::net::TcpListener::bind(gql_addr)
            .await
            .expect("failed to bind GraphQL port");
        let gql_server =
            axum::serve(gql_listener, gql_app).with_graceful_shutdown(shutdown_signal());

        if let Some(sub_port) = subsonic_port {
            let sub_app = super::super::serve::subsonic_router(db_path);
            let sub_addr = std::net::SocketAddr::from(([0, 0, 0, 0], sub_port));
            eprintln!("  Subsonic REST on http://0.0.0.0:{}/rest/", sub_port);

            let sub_listener = tokio::net::TcpListener::bind(sub_addr)
                .await
                .expect("failed to bind Subsonic port");
            let sub_server =
                axum::serve(sub_listener, sub_app).with_graceful_shutdown(shutdown_signal());

            // Run both servers concurrently.
            tokio::select! {
                r = gql_server => r.expect("GraphQL server error"),
                r = sub_server => r.expect("Subsonic server error"),
            }
        } else {
            gql_server.await.expect("server error");
        }
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

/// Run the server as a background daemon.
pub fn cmd_serve_daemon(port: Option<u16>, subsonic_port: Option<u16>, playground: bool) {
    use std::fs;
    use std::process::Command;

    let cfg = Config::load().unwrap_or_default();
    let port_val = port.unwrap_or(cfg.graphql.port);

    let exe = std::env::current_exe().expect("failed to get current exe path");
    let mut cmd = Command::new(exe);
    cmd.arg("serve");
    cmd.arg("--port").arg(port_val.to_string());
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

    eprintln!(
        "koan serve daemon started (pid {}) on port {}",
        pid, port_val
    );
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

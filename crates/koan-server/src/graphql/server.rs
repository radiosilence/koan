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
use crate::auth::routes::{AuthRouteState, LoginRateLimiter, auth_router};

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

    if let Err(e) = run_api_blocking(ApiServerOpts {
        state,
        cmd_tx,
        db_path,
        port,
        bind,
        subsonic_port,
        playground,
        viz: None, // headless — no viz analyzer
    }) {
        eprintln!("koan: {}", e);
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Shared API server logic — used by both headless and TUI+API modes
// ---------------------------------------------------------------------------

/// Ceiling on a single GraphQL query. Anything genuinely longer than this —
/// a library scan, a remote sync — runs as a job instead.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Queries executing at once. Resolvers now do their SQLite and HTTP work on
/// the blocking pool, so this bounds concurrent work rather than protecting the
/// runtime's workers from it.
const MAX_INFLIGHT_QUERIES: usize = 64;

/// Timeout, panic catch and load shed for the query route.
///
/// Not applied to `/graphql/ws`: a subscription is meant to outlive any request
/// timeout.
fn load_perimeter<S>(router: axum::Router<S>) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        // Innermost so it is inside the timeout: a panicking resolver becomes a
        // 500 rather than a silently dropped connection.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        // Shed rather than queue. A concurrency limit on its own parks callers
        // on a semaphore, so an overloaded server answers every client slowly
        // instead of telling the surplus to come back.
        .layer(
            tower::ServiceBuilder::new()
                .layer(axum::error_handling::HandleErrorLayer::new(
                    |err: tower::BoxError| async move {
                        if err.is::<tower::load_shed::error::Overloaded>() {
                            (
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                "server at capacity",
                            )
                        } else {
                            (
                                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                "internal error",
                            )
                        }
                    },
                ))
                .load_shed()
                .concurrency_limit(MAX_INFLIGHT_QUERIES),
        )
}

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
///
/// `Err` means the server refused to start on a misconfiguration the caller has
/// to surface — never a silent downgrade to an unauthenticated server.
fn run_api_blocking(opts: ApiServerOpts) -> Result<(), String> {
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
        let kp = auth::load_keypair().map_err(|e| {
            format!(
                "auth_enabled = true but the keypair could not be loaded: {}. \
                 Run `koan auth setup`.",
                e
            )
        })?;
        // An empty or truncated key file would otherwise leave every request an
        // unauthenticated admin, which is the opposite of what was asked for.
        if kp.0.is_empty() || kp.1.is_empty() {
            return Err("auth_enabled = true but the keypair files are empty. \
                 Run `koan auth regenerate-keys`."
                .into());
        }
        kp
    } else {
        // When auth is disabled, we still need dummy keys for the route state
        // (routes exist but won't be hit by middleware). Generate if available.
        auth::load_or_generate_keypair().unwrap_or_default()
    };

    let access_ttl = parse_duration_secs(&cfg.graphql.access_token_ttl).unwrap_or(900);
    let refresh_ttl = parse_duration_secs(&cfg.graphql.refresh_token_ttl).unwrap_or(2_592_000);

    // Process-scoped introspection key for playground access. It is a bearer
    // credential compared verbatim, so it has to be full-entropy random — a
    // UUID would leak the server start time and cut the guessable space.
    let introspection_key = if playground_enabled && auth_enabled {
        Some(Arc::new(auth::random_token().map_err(|e| {
            format!("failed to generate introspection key: {}", e)
        })?))
    } else {
        None
    };

    let auth_state = AuthState {
        public_pem: Arc::new(public_pem.clone()),
        auth_enabled,
        introspection_key: introspection_key.clone(),
    };

    let auth_route_state = AuthRouteState {
        db_path: db_path.clone(),
        private_pem: Arc::new(private_pem),
        public_pem: Arc::new(public_pem),
        access_ttl_secs: access_ttl,
        refresh_ttl_secs: refresh_ttl,
        cookie_secure: cfg.graphql.cookie_secure,
        login_limiter: Arc::new(LoginRateLimiter::default()),
    };

    let schema = build_schema(state, cmd_tx, db_path.clone(), viz);

    if auth_enabled {
        log::info!(
            "Auth enabled (Ed25519 JWT, access TTL {}s, refresh TTL {}s)",
            access_ttl,
            refresh_ttl
        );
    } else {
        log::info!("Auth disabled — all requests treated as admin");
    }

    let browser_policy = Arc::new(BrowserPolicy {
        origins: cfg.graphql.cors_origins.clone(),
        hosts: cfg.graphql.allowed_hosts.clone(),
    });

    if cfg.graphql.cors_origins.is_empty() {
        log::info!("CORS: no origins configured — browsers get no cross-origin access");
    }

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        // GraphQL routes — protected by auth middleware.
        //
        // The query route carries the load perimeter; the WebSocket route does
        // not, because a subscription is meant to outlive any request timeout.
        let query_route = load_perimeter(axum::Router::new().route("/graphql", post(graphql_handler)));

        let gql_app = axum::Router::new()
            .merge(query_route)
            .route("/graphql/ws", get(graphql_ws_handler))
            .layer(axum::middleware::from_fn_with_state(
                auth_state.clone(),
                auth_middleware,
            ))
            // Runs before auth: a rejected request should never reach a
            // credential check, let alone execute.
            .layer(axum::middleware::from_fn_with_state(
                browser_policy.clone(),
                browser_guard,
            ))
            .with_state(schema);

        // Auth routes — always accessible (no auth middleware).
        let auth_app = auth_router(auth_route_state);

        // CORS. An empty origin list emits no `Access-Control-Allow-Origin` at
        // all: the previous wildcard handed every web page on the internet the
        // ability to read this library.
        let origins: Vec<axum::http::HeaderValue> = cfg
            .graphql
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        let cors = tower_http::cors::CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderName::from_static("x-introspection-key"),
            ])
            .allow_credentials(true);

        // Subsonic REST routes — always mounted on the GraphQL port when
        // remote creds are configured. Previously only available on the
        // dedicated `--subsonic <port>` listener, which broke `koan play
        // --server <url>` because the remote TUI bridge builds its stream
        // URL off the GraphQL base.
        // Built once and cloned: each build re-read the config from disk.
        let subsonic_merged = crate::subsonic::subsonic_router(db_path);
        let subsonic_on_main = subsonic_merged.is_some();
        let subsonic_dedicated = subsonic_merged.clone();

        let mut app = auth_app.merge(gql_app);
        if let Some(sub) = subsonic_merged {
            app = app.merge(sub);
        }
        if playground_enabled {
            app = app.route(
                "/graphql",
                get(graphql_playground).with_state(introspection_key.clone()),
            );
        }
        // Outermost: a request whose `Host` we do not recognise is refused
        // before anything else looks at it. Without this a DNS-rebinding page
        // reaches the API as same-origin and CORS stops mattering.
        let app = app.layer(cors).layer(axum::middleware::from_fn_with_state(
            browser_policy.clone(),
            host_guard,
        ));

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
                if subsonic_on_main {
                    log::info!("Subsonic REST on http://{}:{}/rest/", bind, port);
                }
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
                return Ok(());
            }
        };
        let gql_server = axum::serve(
            gql_listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal());

        // If `--subsonic <port>` is set AND differs from the GraphQL port,
        // run an additional dedicated listener. This preserves the old
        // behavior for users who want Subsonic on its own port.
        let extra_sub_port = subsonic_port.filter(|p| *p != port);
        if let Some(sub_port) = extra_sub_port
            && let Some(sub_app) = subsonic_dedicated
        {
            let sub_addr = std::net::SocketAddr::new(bind, sub_port);
            match tokio::net::TcpListener::bind(sub_addr).await {
                Ok(sub_listener) => {
                    log::info!(
                        "Subsonic REST also on http://{}:{}/rest/ (dedicated port)",
                        bind,
                        sub_port,
                    );
                    let sub_server = axum::serve(sub_listener, sub_app)
                        .with_graceful_shutdown(shutdown_signal());

                    tokio::select! {
                        r = gql_server => { if let Err(e) = r { log::error!("GraphQL server error: {e}"); } },
                        r = sub_server => { if let Err(e) = r { log::error!("Subsonic server error: {e}"); } },
                    }
                    return Ok(());
                }
                Err(e) => {
                    log::warn!(
                        "Dedicated Subsonic port {} unavailable — {}. Mounted on GraphQL port only.",
                        sub_port,
                        e,
                    );
                }
            }
        }

        if let Err(e) = gql_server.await {
            log::error!("GraphQL server error: {e}");
        }
        Ok(())
    })
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
    // Runs on a spawned thread in TUI mode, where a panic would take the API
    // down with nothing on screen to say so.
    if let Err(e) = run_api_blocking(ApiServerOpts {
        state,
        cmd_tx,
        db_path,
        port,
        bind,
        subsonic_port,
        playground,
        viz: None,
    }) {
        log::error!("API server not started: {}", e);
    }
}

// ---------------------------------------------------------------------------
// Browser perimeter
// ---------------------------------------------------------------------------

/// What this server will answer to when the caller is a browser.
///
/// Two separate questions: which `Host` values name this server (DNS rebinding),
/// and which `Origin` values may talk to it (CSRF, cross-site WebSockets).
pub(crate) struct BrowserPolicy {
    origins: Vec<String>,
    hosts: Vec<String>,
}

impl BrowserPolicy {
    fn host_allowed(&self, host: &str) -> bool {
        if self.hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
            return true;
        }
        let bare = strip_port(host);
        if self.hosts.iter().any(|h| h.eq_ignore_ascii_case(bare)) {
            return true;
        }
        // A rebinding attack needs a name it controls; literals and localhost
        // resolve to this machine by definition.
        bare.eq_ignore_ascii_case("localhost") || bare.parse::<std::net::IpAddr>().is_ok()
    }

    /// An origin is allowed if it is configured, or if it is simply this server
    /// talking to itself — which is what the bundled playground does.
    fn origin_allowed(&self, origin: &str, host: Option<&str>) -> bool {
        if self.origins.iter().any(|o| o == origin) {
            return true;
        }
        match (origin.split_once("://"), host) {
            (Some((_, authority)), Some(host)) => authority.eq_ignore_ascii_case(host),
            _ => false,
        }
    }
}

/// `example.com:4000` -> `example.com`, `[::1]:4000` -> `::1`.
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

fn header_str(request: &axum::extract::Request, name: axum::http::HeaderName) -> Option<&str> {
    request.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Reject requests carrying an unrecognised `Host`.
async fn host_guard(
    axum::extract::State(policy): axum::extract::State<Arc<BrowserPolicy>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    // No `Host` at all means no browser: only HTTP/1.0 and raw tooling omit it,
    // and neither can be steered by an attacker page.
    let host = header_str(&request, axum::http::header::HOST)
        .map(str::to_owned)
        .or_else(|| request.uri().host().map(str::to_owned));

    if let Some(ref host) = host
        && !policy.host_allowed(host)
    {
        log::warn!("rejected request for unrecognised Host: {}", host);
        return (axum::http::StatusCode::FORBIDDEN, "host not allowed").into_response();
    }

    next.run(request).await
}

/// Reject cross-site GraphQL traffic.
///
/// Two holes, one guard. A WebSocket handshake is exempt from CORS entirely, so
/// a foreign page can open `/graphql/ws`, have the browser attach the session
/// cookie, and read every response. And a POST whose content type is
/// CORS-safelisted (`text/plain`) is sent without a preflight, yet
/// async-graphql parses it as JSON regardless — so the mutation lands even
/// though the reply is unreadable.
async fn browser_guard(
    axum::extract::State(policy): axum::extract::State<Arc<BrowserPolicy>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let host = header_str(&request, axum::http::header::HOST).map(str::to_owned);
    // No `Origin` means a non-browser client, which CSRF cannot reach.
    if let Some(origin) = header_str(&request, axum::http::header::ORIGIN)
        && !policy.origin_allowed(origin, host.as_deref())
    {
        log::warn!(
            "rejected GraphQL request from disallowed Origin: {}",
            origin
        );
        return (axum::http::StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }

    if request.method() == axum::http::Method::POST && !is_graphql_content_type(&request) {
        return (
            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content type must be application/json or application/graphql",
        )
            .into_response();
    }

    next.run(request).await
}

fn is_graphql_content_type(request: &axum::extract::Request) -> bool {
    header_str(request, axum::http::header::CONTENT_TYPE).is_some_and(|ct| {
        let ct = ct.trim().to_ascii_lowercase();
        ct.starts_with("application/json") || ct.starts_with("application/graphql")
    })
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
///
/// There is no credential to check, so the caller states the role it wants the
/// query executed at — see `mcp::mcp_role`.
pub async fn execute_in_process(
    schema: &KoanSchema,
    query: &str,
    variables: Option<serde_json::Value>,
    role: koan_core::auth::Role,
) -> serde_json::Value {
    let mut request = async_graphql::Request::new(query);
    request = request.data(AuthUser {
        role,
        ..AuthUser::anonymous_admin()
    });
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::{get, post};
    use tower::ServiceExt as _;

    fn policy() -> Arc<BrowserPolicy> {
        Arc::new(BrowserPolicy {
            origins: vec!["https://music.example.com".into()],
            hosts: vec!["koan.local".into()],
        })
    }

    async fn ok() -> &'static str {
        "ok"
    }

    fn routes() -> axum::Router<Arc<BrowserPolicy>> {
        axum::Router::new()
            .route("/graphql", post(ok).get(ok))
            .route("/graphql/ws", get(ok))
    }

    async fn run_host(req: HttpRequest<Body>) -> StatusCode {
        let app = routes()
            .layer(axum::middleware::from_fn_with_state(policy(), host_guard))
            .with_state(policy());
        app.oneshot(req).await.unwrap().status()
    }

    async fn run_browser(req: HttpRequest<Body>) -> StatusCode {
        let app = routes()
            .layer(axum::middleware::from_fn_with_state(
                policy(),
                browser_guard,
            ))
            .with_state(policy());
        app.oneshot(req).await.unwrap().status()
    }

    fn json_post(uri: &str) -> axum::http::request::Builder {
        HttpRequest::post(uri).header(axum::http::header::CONTENT_TYPE, "application/json")
    }

    // -- Host allowlist (DNS rebinding) --

    #[test]
    fn host_policy_accepts_loopback_literals_and_configured_names() {
        let p = policy();
        assert!(p.host_allowed("localhost:4000"));
        assert!(p.host_allowed("127.0.0.1:4000"));
        assert!(p.host_allowed("192.168.1.20:4000"));
        assert!(p.host_allowed("[::1]:4000"));
        assert!(p.host_allowed("koan.local"));
        assert!(p.host_allowed("koan.local:4000"));
    }

    #[test]
    fn host_policy_rejects_attacker_controlled_names() {
        let p = policy();
        assert!(!p.host_allowed("evil.com"));
        assert!(!p.host_allowed("rebind.evil.com:4000"));
        assert!(!p.host_allowed("koan.local.evil.com"));
    }

    #[tokio::test]
    async fn host_guard_rejects_foreign_host() {
        let req = json_post("/graphql")
            .header(axum::http::header::HOST, "rebind.evil.com")
            .body(Body::empty())
            .unwrap();
        assert_eq!(run_host(req).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn host_guard_allows_known_host_and_missing_host() {
        let req = json_post("/graphql")
            .header(axum::http::header::HOST, "127.0.0.1:4000")
            .body(Body::empty())
            .unwrap();
        assert_eq!(run_host(req).await, StatusCode::OK);

        let req = json_post("/graphql").body(Body::empty()).unwrap();
        assert_eq!(run_host(req).await, StatusCode::OK);
    }

    // -- Cross-site WebSocket --

    #[tokio::test]
    async fn ws_upgrade_from_foreign_origin_is_rejected() {
        let req = HttpRequest::get("/graphql/ws")
            .header(axum::http::header::HOST, "127.0.0.1:4000")
            .header(axum::http::header::ORIGIN, "https://evil.com")
            .body(Body::empty())
            .unwrap();
        assert_eq!(run_browser(req).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ws_upgrade_without_origin_is_allowed() {
        let req = HttpRequest::get("/graphql/ws")
            .header(axum::http::header::HOST, "127.0.0.1:4000")
            .body(Body::empty())
            .unwrap();
        assert_eq!(run_browser(req).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn configured_and_same_origin_are_allowed() {
        let req = HttpRequest::get("/graphql/ws")
            .header(axum::http::header::HOST, "127.0.0.1:4000")
            .header(axum::http::header::ORIGIN, "https://music.example.com")
            .body(Body::empty())
            .unwrap();
        assert_eq!(run_browser(req).await, StatusCode::OK);

        // The bundled playground posts to the host it was served from.
        let req = json_post("/graphql")
            .header(axum::http::header::HOST, "127.0.0.1:4000")
            .header(axum::http::header::ORIGIN, "http://127.0.0.1:4000")
            .body(Body::empty())
            .unwrap();
        assert_eq!(run_browser(req).await, StatusCode::OK);
    }

    // -- CSRF via a CORS-safelisted content type --

    #[tokio::test]
    async fn text_plain_post_is_rejected() {
        let req = HttpRequest::post("/graphql")
            .header(axum::http::header::CONTENT_TYPE, "text/plain")
            .body(Body::from(r#"{"query":"mutation{clearQueue{ok}}"}"#))
            .unwrap();
        assert_eq!(run_browser(req).await, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn post_without_content_type_is_rejected() {
        let req = HttpRequest::post("/graphql").body(Body::empty()).unwrap();
        assert_eq!(run_browser(req).await, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    // -- Load perimeter --

    #[tokio::test]
    async fn load_perimeter_passes_requests_and_turns_panics_into_500s() {
        async fn boom() -> &'static str {
            panic!("resolver exploded");
        }

        let app = load_perimeter(
            axum::Router::new()
                .route("/graphql", post(ok))
                .route("/boom", post(boom)),
        );

        let req = json_post("/graphql").body(Body::empty()).unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::OK
        );

        // Without CatchPanicLayer this drops the connection with nothing logged.
        let req = json_post("/boom").body(Body::empty()).unwrap();
        assert_eq!(
            app.oneshot(req).await.unwrap().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn json_post_is_accepted() {
        let req = json_post("/graphql").body(Body::empty()).unwrap();
        assert_eq!(run_browser(req).await, StatusCode::OK);

        let req = HttpRequest::post("/graphql")
            .header(
                axum::http::header::CONTENT_TYPE,
                "application/json; charset=utf-8",
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(run_browser(req).await, StatusCode::OK);
    }
}

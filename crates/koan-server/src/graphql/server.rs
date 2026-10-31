use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::audio::viz::VizSnapshot;
use koan_core::auth::{self, parse_duration_secs};
use koan_core::config::Config;
use koan_core::db::pool::Pool;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::SharedPlayerState;

use super::KoanSchema;
use crate::auth::AuthUser;
use crate::auth::middleware::{AuthState, auth_middleware};
use crate::auth::routes::{AuthRouteState, RateLimiter, auth_router};

// ---------------------------------------------------------------------------
// `koan --headless` entry point (standalone headless server)
// ---------------------------------------------------------------------------

/// A server hands out links to itself — invites, shares, sign-ins — and is
/// known to its clients by one address, which it cannot work out for itself.
fn require_server_url() {
    let why = match Config::cached().server_url() {
        None => "server.url is not set. Set it, or KOAN_SERVER__URL, to the address clients reach this server at, e.g. https://koan.example.com".to_owned(),
        Some(url) if !url.starts_with("https://") && !url.starts_with("http://") => {
            format!("server.url is {url:?}, which is not an http:// or https:// address")
        }
        Some(_) => return,
    };
    log::error!("{why}");
    eprintln!("koan: {why}");
    std::process::exit(1);
}

pub fn cmd_serve(
    port: Option<u16>,
    bind: Option<std::net::IpAddr>,
    subsonic_port: Option<u16>,
    playground: bool,
) {
    use koan_core::player::Player;

    require_server_url();
    // Open, and migrate, before binding: a database this build cannot read,
    // such as one a newer koan has migrated, stops it here with the reason
    // rather than serving errors.
    if let Err(e) = koan_core::db::connection::Database::open_default() {
        log::error!("cannot open the database: {e}");
        eprintln!("koan: cannot open the database: {e}");
        std::process::exit(1);
    }
    let db_path = koan_core::config::db_path();
    let pool = Arc::new(Pool::new(db_path.clone()));
    koan_core::db::pool::on_outdated(|| OUTDATED.cancel());

    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    // A server has no one to press "scan": index at start and whenever the
    // library folders change, as the macOS app does.
    let watched = db_path.clone();
    let jobs = super::jobs::JobRegistry::default();
    let record = super::jobs::record_watch(&jobs);
    koan_core::helpers::spawn_library_watch(db_path, move |scan| {
        record(&scan);
        // A scan that just finished may have brought in an album someone asked
        // to have queued when it arrived.
        if let koan_core::helpers::WatchScan::Finished(_) = scan {
            crate::clients::fulfil_from(&watched);
            if let Ok(db) = koan_core::db::connection::Database::open_existing(&watched) {
                crate::clients::changed_if_library_moved(&db.conn);
            }
        }
    });

    let started = ApiServer::start_with(
        ApiServerOpts {
            state,
            cmd_tx,
            pool,
            port,
            bind,
            subsonic_port,
            playground,
            viz: None, // headless — no viz analyzer
            profile: Profile::Headless,
            hosts: Vec::new(),
        },
        jobs,
    );
    match started {
        Ok(server) => server.wait(),
        Err(e) => {
            eprintln!("koan: {}", e);
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Shared API server logic — used by both headless and TUI+API modes
// ---------------------------------------------------------------------------

/// Ceiling on a single GraphQL query. Anything longer than this —
/// a library scan, a remote sync — runs as a job instead.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Queries executing at once. Resolvers do their SQLite and HTTP work on the
/// blocking pool, so this bounds concurrent work rather than protecting the
/// runtime's workers from it.
const MAX_INFLIGHT_QUERIES: usize = 64;

/// Largest query body accepted. async-graphql reads the whole body before
/// parsing it, with no limit of its own. A query is text; even a playlist of
/// thousands of ids is a fraction of this.
const MAX_QUERY_BODY: usize = 2 << 20;

/// Timeout, panic catch, body limit and load shed for the query route.
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
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            MAX_QUERY_BODY,
        ))
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

/// Who the server runs for, which decides what it takes from the `[graphql]`,
/// `[subsonic]` and `[server]` settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// `koan --headless`: a server and nothing else. Its player is heard by
    /// nobody.
    Headless,
    /// Beside the TUI, on the TUI's player.
    Tui,
    /// Inside an app sharing its library, on the app's player. Subsonic is
    /// always on and only accounts sign in to it. The app owns the process, so
    /// signals are left to it, and the settings a headless deployment uses
    /// (demo, proxy sign-in, the playground) do not apply.
    Embedded,
}

/// Options for the API server — avoids too-many-arguments.
pub struct ApiServerOpts {
    pub state: Arc<SharedPlayerState>,
    pub cmd_tx: Sender<PlayerCommand>,
    /// Shared by every route that reads the library outside GraphQL, and by
    /// the MCP listener when there is one.
    pub pool: Arc<Pool>,
    pub port: Option<u16>,
    pub bind: Option<std::net::IpAddr>,
    pub subsonic_port: Option<u16>,
    pub playground: bool,
    pub viz: Option<Arc<VizSnapshot>>,
    pub profile: Profile,
    /// Names this server is reached by beyond `graphql.allowed_hosts`, such as
    /// the Mac's Bonjour name.
    pub hosts: Vec<String>,
}

/// A running API server. Dropping it asks the server to stop and does not
/// wait for it.
pub struct ApiServer {
    addr: std::net::SocketAddr,
    /// What the app hosting an embedded server administers it with: an API
    /// key good for the first admin, made for this run and never written down.
    host_key: Option<String>,
    stop: tokio_util::sync::CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ApiServer {
    /// Bind the listeners, then serve on a thread of its own.
    ///
    /// `Err` means the server refused to start, on a misconfiguration or a port
    /// it could not bind, and the caller has to surface it — never a silent
    /// downgrade to an unauthenticated server. Nothing process-wide is touched
    /// until the main port is bound.
    ///
    /// `jobs` is made by the caller, so a library watch started before the
    /// server records its scans from the first.
    pub fn start(opts: ApiServerOpts) -> Result<Self, String> {
        Self::start_with(opts, Default::default())
    }

    pub(crate) fn start_with(
        opts: ApiServerOpts,
        jobs: super::jobs::JobRegistry,
    ) -> Result<Self, String> {
        let ApiServerOpts {
            state,
            cmd_tx,
            pool,
            port,
            bind,
            subsonic_port,
            playground,
            viz,
            profile,
            hosts,
        } = opts;
        use axum::routing::{get, post};

        let cfg = Config::cached();
        let embedded = profile == Profile::Embedded;
        let port = port.unwrap_or(cfg.graphql.port);
        // The wizard is for a server with no shell to hand, in a container. One
        // beside a terminal session or inside an app would open its /setup to
        // any local process with nothing on screen to say so.
        let setup_wizard = cfg.graphql.setup_wizard && profile == Profile::Headless;
        let bind = bind.unwrap_or(cfg.graphql.bind);
        // An app's server has one port, the one it was given.
        let subsonic_port = if embedded {
            None
        } else {
            subsonic_port.or(cfg.subsonic.port)
        };
        let subsonic_enabled = embedded || cfg.subsonic.enabled;
        let playground_enabled = !embedded && (playground || cfg.graphql.playground);
        let auth_enabled = cfg.graphql.auth_enabled;
        let demo = cfg.server.demo;

        if embedded && !auth_enabled {
            return Err(
                "graphql.auth_enabled is off, which would make anyone who reaches \
                 this port an admin"
                    .into(),
            );
        }
        if demo && embedded {
            return Err("server.demo is for a headless server".into());
        }
        // A demo is one shared account; without sign-in everyone would be admin.
        if demo && !auth_enabled {
            return Err("server.demo needs graphql.auth_enabled".into());
        }

        // Load or generate Ed25519 keypair for JWT signing. A server's first
        // start makes its own: it is this server's signing key and nothing
        // else, a fresh one invalidates no token (there can be none yet), and a
        // server in a container has no terminal to run `koan auth setup` in
        // before it starts. Accounts are still created deliberately; until one
        // exists, nothing signs in.
        let (private_pem, public_pem) = if auth_enabled {
            let kp = auth::load_or_generate_keypair().map_err(|e| {
                format!(
                    "auth_enabled = true but the keypair could not be loaded or created: {}",
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

        let proxy_auth = if embedded {
            None
        } else {
            crate::ui::ProxyAuth::from_config(
                &cfg.graphql.proxy_auth_header,
                &cfg.graphql.proxy_auth_from,
            )?
        };

        let gql_listener = std::net::TcpListener::bind((bind, port))
            .and_then(|l| l.set_nonblocking(true).map(|()| l))
            .map_err(|e| {
                format!("failed to bind GraphQL port {port} — {e} (another instance running?)")
            })?;
        let addr = gql_listener
            .local_addr()
            .map_err(|e| format!("failed to read the GraphQL listener's address — {e}"))?;

        // `--subsonic <port>` set to something other than the GraphQL port adds
        // a dedicated Subsonic listener.
        let sub_listener = subsonic_port
            .filter(|p| subsonic_enabled && *p != port)
            .and_then(|sub_port| {
                match std::net::TcpListener::bind((bind, sub_port))
                    .and_then(|l| l.set_nonblocking(true).map(|()| l))
                {
                    Ok(l) => Some(l),
                    Err(e) => {
                        log::warn!(
                            "Dedicated Subsonic port {sub_port} unavailable — {e}. Mounted on GraphQL port only."
                        );
                        None
                    }
                }
            });

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| format!("could not start the API server's runtime — {e}"))?;
        let (gql_listener, sub_listener) = {
            let _in = rt.enter();
            let gql = tokio::net::TcpListener::from_std(gql_listener)
                .map_err(|e| format!("failed to listen on {addr} — {e}"))?;
            let sub = sub_listener.and_then(|l| match tokio::net::TcpListener::from_std(l) {
                Ok(l) => Some(l),
                Err(e) => {
                    log::warn!("Dedicated Subsonic listener unusable — {e}");
                    None
                }
            });
            (gql, sub)
        };

        let host_key = embedded
            .then(auth::random_token)
            .transpose()
            .map_err(|e| format!("failed to generate the host key: {e}"))?;

        if demo {
            let db = pool.get().map_err(|e| format!("demo: {e}"))?;
            crate::demo::start(&db.conn).map_err(|e| format!("demo: {e}"))?;
        }

        // What once encrypted account passwords for Subsonic token auth. Nothing
        // is encrypted with it any more; a copy left on disk is a liability.
        let _ = std::fs::remove_file(auth::keypair_dir().join("subsonic.key"));

        crate::auth::set_signing_keys(Arc::new(private_pem.clone()), Arc::new(public_pem.clone()));

        let auth_state = AuthState {
            public_pem: Arc::new(public_pem.clone()),
            auth_enabled,
            introspection_key: introspection_key.clone(),
            pool: pool.clone(),
        };

        // One for every door a password comes through, so its limits are shared.
        let users = Arc::new(crate::auth::password::PasswordVerifier::new(pool.clone()));
        let auth_route_state = AuthRouteState {
            users: users.clone(),
            pool: pool.clone(),
            private_pem: Arc::new(private_pem),
            public_pem: Arc::new(public_pem),
            access_ttl_secs: access_ttl,
            refresh_ttl_secs: refresh_ttl,
            cookie_secure: cfg.graphql.cookie_secure,
            login_limiter: Arc::new(RateLimiter::default()),
        };

        // Plays accounts forward to ListenBrainz; it sleeps while none do.
        koan_core::scrobbling::start(pool.path().to_path_buf());
        // Made here, off the runtime: the relay's client is a blocking one,
        // which cannot be built inside it, and the routes ask for it there.
        let _ = crate::push::pusher();

        // Ends graceful shutdown, every open session and the MCP streams.
        let stop = tokio_util::sync::CancellationToken::new();
        // An app sharing its library serves its own apps; the web UI and MCP
        // are for a server people reach with a browser or an assistant.
        let mcp_routes = (!embedded).then(|| {
            crate::mcp::router(
                state.clone(),
                cmd_tx.clone(),
                auth_state.clone(),
                cfg.server_url(),
                profile == Profile::Headless,
                stop.clone(),
            )
        });
        let schema = super::build_schema_sharing(state, cmd_tx, pool.clone(), viz, jobs.clone());

        if auth_enabled {
            log::info!(
                "Auth enabled (Ed25519 JWT, access TTL {}s, refresh TTL {}s)",
                access_ttl,
                refresh_ttl
            );
            let no_accounts = pool
                .get()
                .ok()
                .and_then(|db| koan_core::db::queries::auth::has_users(&db.conn).ok())
                == Some(false);
            if no_accounts && setup_wizard {
                log::warn!(
                    "No accounts yet: whoever opens /setup first becomes the admin. \
                     Set graphql.setup_wizard = false to make it with `koan auth setup` instead."
                );
            }
        } else {
            log::info!("Auth disabled — all requests treated as admin");
        }

        let mut allowed_hosts = cfg.graphql.allowed_hosts.clone();
        allowed_hosts.extend(hosts);
        // On a home network every device has a private address; only a tunnel
        // client on this machine forwards for anyone.
        let forwarded_from = if embedded {
            crate::auth::routes::ForwardedFrom::Loopback
        } else {
            crate::auth::routes::ForwardedFrom::Private
        };
        let browser_policy = Arc::new(BrowserPolicy {
            origins: cfg.graphql.cors_origins.clone(),
            hosts: allowed_hosts,
        });

        if cfg.graphql.cors_origins.is_empty() {
            log::info!("CORS: no origins configured — browsers get no cross-origin access");
        }

        let serving = stop.clone();
        let serving_key = host_key.clone();
        let thread = std::thread::Builder::new()
            .name("koan-api".into())
            .spawn(move || {
                let stop = serving;
                rt.block_on(async {
                    if !embedded {
                        let stop = stop.clone();
                        tokio::spawn(async move {
                            shutdown_signal().await;
                            stop.cancel();
                        });
                    }

                    // GraphQL routes — protected by auth middleware.
                    //
                    // The query route carries the load perimeter; the WebSocket route does
                    // not, because a subscription is meant to outlive any request timeout.
                    let query_route =
                        load_perimeter(axum::Router::new().route("/graphql", post(graphql_handler)));

                    let gql_app = axum::Router::new()
                        .merge(query_route)
                        .route("/graphql/ws", get(graphql_ws_handler))
                        .layer(axum::Extension(stop.clone()))
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

                    // The web UI checks its own session (a page load without one is sent
                    // to sign in rather than refused), so it sits outside the GraphQL auth
                    // layer. Built before the auth routes take their state.
                    let covers = Arc::new(crate::covers::Covers::in_config_dir());
                    let ui_routes = (!embedded).then(|| crate::ui::router(
                        pool.clone(),
                        auth_route_state.clone(),
                        auth_enabled,
                        setup_wizard,
                        covers.clone(),
                        cfg.server_url(),
                        cfg.mcp.redirect_hosts.clone(),
                        proxy_auth,
                        jobs,
                    )
                    // A template that fails to render panics; this makes it a 500.
                    .layer(tower_http::catch_panic::CatchPanicLayer::new()));
                    let ui_routes = ui_routes.map(|ui| {
                        if demo {
                            ui.layer(axum::middleware::from_fn(crate::demo::guard))
                        } else {
                            ui
                        }
                    });

                    // Auth routes — always accessible (no auth middleware).
                    let auth_app = auth_router(auth_route_state);

                    // CORS. An empty origin list emits no `Access-Control-Allow-Origin` at
                    // all; a wildcard would let any web page read this library.
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

                    // Public by design, so outside the auth layers: each route answers for
                    // one share's own tracks, or the one cover a notification's signed
                    // link names, and nothing else. The Host guard still applies.
                    let share_routes =
                        crate::share::router(pool.clone(), cfg.server_url(), covers.clone())
                            .layer(tower_http::catch_panic::CatchPanicLayer::new());
                    // Subsonic on the GraphQL port whenever it is enabled: the remote
                    // TUI bridge builds its stream URL off the GraphQL base. Built once
                    // and cloned for the dedicated listener, since each build reads the
                    // config from disk.
                    // The demo guard goes on Subsonic itself, so a dedicated Subsonic
                    // port has it too, and on the web UI above.
                    let subsonic_merged = subsonic_enabled
                        .then(|| {
                            crate::subsonic::subsonic_router(
                                pool,
                                covers,
                                users,
                                profile,
                                stop.clone(),
                                serving_key,
                            )
                        })
                        .map(|sub| {
                            if demo {
                                sub.layer(axum::middleware::from_fn(crate::demo::guard))
                            } else {
                                sub
                            }
                        });
                    let subsonic_dedicated = subsonic_merged.clone();

                    let mut app = auth_app.merge(gql_app).merge(share_routes);
                    if let Some(ui) = ui_routes {
                        app = app.merge(ui);
                    }
                    if let Some(mcp) = mcp_routes {
                        app = app.merge(mcp);
                    }
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
                    let app = app
                        .layer(cors)
                        .layer(axum::middleware::from_fn_with_state(
                            browser_policy.clone(),
                            host_guard,
                        ))
                        .layer(axum::Extension(forwarded_from));

                    log::info!("GraphQL API on http://{addr}/graphql");
                    if subsonic_enabled {
                        log::info!("Subsonic REST on http://{addr}/rest/");
                    }
                    if playground_enabled {
                        let playground_url = match introspection_key {
                            Some(ref key) => {
                                format!("http://{addr}/graphql?introspection-key={key}")
                            }
                            None => format!("http://{addr}/graphql"),
                        };
                        log::info!("GraphiQL: {}", playground_url);
                        // Open browser on macOS/Linux.
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("open").arg(&playground_url).spawn();
                        #[cfg(target_os = "linux")]
                        let _ = std::process::Command::new("xdg-open")
                            .arg(&playground_url)
                            .spawn();
                    }
                    let gql_server = axum::serve(
                        gql_listener,
                        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                    )
                    .with_graceful_shutdown(stop.clone().cancelled_owned());

                    if let Some((sub_listener, sub_app)) = sub_listener.zip(subsonic_dedicated) {
                        if let Ok(a) = sub_listener.local_addr() {
                            log::info!("Subsonic REST also on http://{a}/rest/ (dedicated port)");
                        }
                        // With connection info, as the main listener: the sign-in
                        // throttle keys on the client's address.
                        let sub_server = axum::serve(
                            sub_listener,
                            sub_app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                        )
                        .with_graceful_shutdown(stop.clone().cancelled_owned());

                        tokio::select! {
                            r = gql_server => { if let Err(e) = r { log::error!("GraphQL server error: {e}"); } },
                            r = sub_server => { if let Err(e) = r { log::error!("Subsonic server error: {e}"); } },
                            _ = drain_deadline(&stop) => log::info!("shutting down with connections still open"),
                        }
                        return;
                    }

                    tokio::select! {
                        r = gql_server => { if let Err(e) = r { log::error!("GraphQL server error: {e}"); } },
                        _ = drain_deadline(&stop) => log::info!("shutting down with connections still open"),
                    }
                });
                // Blocking work still running (a scan job, a stream's file read)
                // is left to finish on its own rather than holding up the stop.
                rt.shutdown_timeout(DRAIN);
            })
            .map_err(|e| format!("could not start the API server thread — {e}"))?;

        Ok(Self {
            addr,
            host_key,
            stop,
            thread: Some(thread),
        })
    }

    /// See `host_key` on the struct. `None` but for an embedded server.
    pub fn host_key(&self) -> Option<&str> {
        self.host_key.as_deref()
    }

    /// The address the main listener is bound to.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// Stop accepting, close open sessions, and wait for the server to finish:
    /// at most `DRAIN` for connections, then `DRAIN` for blocking work.
    pub fn stop(mut self) {
        self.stop.cancel();
        self.join();
    }

    /// Wait until the server stops on its own: on a signal, for the profiles
    /// that listen for them.
    pub fn wait(mut self) {
        self.join();
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("API server thread panicked");
        }
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Start the API server beside the TUI, blocking until it stops.
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
    let started = ApiServer::start(ApiServerOpts {
        state,
        cmd_tx,
        pool: Arc::new(Pool::new(db_path)),
        port,
        bind,
        subsonic_port,
        playground,
        viz: None,
        profile: Profile::Tui,
        hosts: Vec::new(),
    });
    match started {
        Ok(server) => server.wait(),
        Err(e) => log::error!("API server not started: {}", e),
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

/// Cancelled when the database turns out to have been upgraded by a newer
/// koan, as the one replacing this server does when it starts: this build
/// can no longer serve it, so it drains and exits as on SIGTERM.
static OUTDATED: std::sync::LazyLock<tokio_util::sync::CancellationToken> =
    std::sync::LazyLock::new(tokio_util::sync::CancellationToken::new);

/// Resolves on SIGINT, SIGTERM where there is one, or the database being
/// upgraded past this build. SIGTERM is how a service manager stops a
/// process, and as PID 1 in a container an unhandled one is dropped by the
/// kernel, leaving the server running until it is killed.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("failed to listen for SIGTERM");
        tokio::select! {
            r = tokio::signal::ctrl_c() => r.expect("failed to listen for ctrl+c"),
            _ = terminate.recv() => {}
            _ = OUTDATED.cancelled() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::select! {
        r = tokio::signal::ctrl_c() => r.expect("failed to listen for ctrl+c"),
        _ = OUTDATED.cancelled() => {}
    }
}

/// How long shutdown waits for open connections before giving up on them.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Resolves `DRAIN` after the server is told to stop. Graceful shutdown waits for every
/// connection to close, and subscription websockets and audio streams never
/// close on their own, so without a deadline the wait ends only in a kill.
async fn drain_deadline(stop: &tokio_util::sync::CancellationToken) {
    stop.cancelled().await;
    tokio::time::sleep(DRAIN).await;
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

/// Subscriptions over a WebSocket, as the account the upgrade request
/// authenticated. Closed when that account changes or its token lapses (see
/// `Lease`), and when the server stops; otherwise, as when auth is off, it
/// stays open.
async fn graphql_ws_handler(
    axum::Extension(user): axum::Extension<AuthUser>,
    lease: Option<axum::Extension<crate::auth::Lease>>,
    axum::Extension(stop): axum::Extension<tokio_util::sync::CancellationToken>,
    axum::extract::State(schema): axum::extract::State<KoanSchema>,
    protocol: async_graphql_axum::GraphQLProtocol,
    websocket: axum::extract::WebSocketUpgrade,
) -> axum::response::Response {
    use axum::extract::ws::{CloseFrame, Message, close_code};
    use futures_util::{SinkExt, StreamExt};
    websocket
        .protocols(async_graphql::http::ALL_WEBSOCKET_PROTOCOLS)
        .on_upgrade(move |socket| async move {
            let (mut sink, stream) = socket.split();
            let serve = async_graphql_axum::GraphQLWebSocket::new_with_pair(
                &mut sink, stream, schema, protocol,
            )
            .on_connection_init(move |_| async move {
                let mut data = async_graphql::Data::default();
                data.insert(user);
                Ok(data)
            })
            .serve();
            let ended = async {
                match lease {
                    Some(axum::Extension(lease)) => lease.ended().await,
                    None => std::future::pending().await,
                }
            };
            let close = tokio::select! {
                _ = serve => None,
                _ = ended => Some((close_code::NORMAL, "sign in again")),
                _ = stop.cancelled() => Some((close_code::AWAY, "server stopping")),
            };
            if let Some((code, reason)) = close {
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code,
                        reason: reason.into(),
                    })))
                    .await;
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

    require_server_url();

    let cfg = Config::cached();
    let port_val = port.unwrap_or(cfg.graphql.port);
    let bind_val = bind.unwrap_or(cfg.graphql.bind);

    let exe = std::env::current_exe().expect("failed to get current exe path");
    let mut cmd = Command::new(exe);

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

    eprintln!("kōan daemon started (pid {}) on port {}", pid, port_val);
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
/// There is no credential to check, so the caller states who the query runs as
/// and at what role — see `mcp::mcp_role`.
pub async fn execute_in_process(
    schema: &KoanSchema,
    query: &str,
    variables: Option<serde_json::Value>,
    caller: AuthUser,
) -> serde_json::Value {
    let mut request = async_graphql::Request::new(query).data(caller);
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
    async fn load_perimeter_refuses_an_oversized_query_body() {
        async fn parse(_: async_graphql_axum::GraphQLRequest) -> StatusCode {
            StatusCode::OK
        }
        let app = load_perimeter(axum::Router::new().route("/graphql", post(parse)));
        // A valid query padded with whitespace, streamed so no Content-Length
        // warns the limit ahead of the bytes.
        let body = |padding: usize| {
            let chunks = [
                axum::body::Bytes::from_static(br#"{"query":"{__typename}""#),
                axum::body::Bytes::from(vec![b' '; padding]),
                axum::body::Bytes::from_static(b"}"),
            ];
            Body::from_stream(tokio_stream::iter(chunks.map(Ok::<_, std::io::Error>)))
        };
        let req = json_post("/graphql").body(body(1 << 10)).unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::OK
        );
        let req = json_post("/graphql").body(body(3 << 20)).unwrap();
        assert_ne!(app.oneshot(req).await.unwrap().status(), StatusCode::OK);
    }

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

    /// A server in an app is stopped and started again within one process:
    /// a stop ends its links rather than waiting them out, and leaves nothing
    /// behind that the next start trips over.
    #[test]
    fn an_embedded_server_ends_its_links_on_stop_and_starts_again() {
        use koan_core::auth::Role;
        use koan_core::db::connection::Database;
        use koan_core::player::commands::CommandChannel;

        let _own = koan_core::config::SWITCHING
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        koan_core::config::set_config_dir(dir.path());
        let db_path = dir.path().join("koan.db");
        let db = Database::open(&db_path).unwrap();
        koan_core::db::schema::create_tables(&db.conn).unwrap();
        koan_core::db::queries::auth::create_user(&db.conn, "sharer", "hunter22", Role::Admin)
            .unwrap();

        let start = || {
            ApiServer::start(ApiServerOpts {
                state: SharedPlayerState::new(),
                cmd_tx: CommandChannel::new().tx,
                pool: Arc::new(Pool::new(db_path.clone())),
                port: Some(0),
                bind: Some(std::net::Ipv4Addr::LOCALHOST.into()),
                subsonic_port: None,
                playground: false,
                viz: None,
                profile: Profile::Embedded,
                hosts: Vec::new(),
            })
            .unwrap()
        };
        // Subsonic answers, though `[subsonic]` is off by default.
        let ping = |addr: std::net::SocketAddr| {
            let body = reqwest::blocking::get(format!(
                "http://{addr}/rest/ping?u=sharer&p=hunter22&v=1.16.1&c=test&f=json"
            ))
            .unwrap()
            .text()
            .unwrap();
            assert!(body.contains(r#""status":"ok""#), "{body}");
        };
        let linked = || {
            crate::clients::registry()
                .list(Some("sharer"))
                .iter()
                .any(|c| c.device == "embedded-restart")
        };

        let server = start();
        let addr = server.addr();
        ping(addr);
        let (mut socket, _) = tungstenite::connect(format!(
            "ws://{addr}/rest/koanLink?u=sharer&p=hunter22&v=1.16.1&c=test&device=embedded-restart"
        ))
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !linked() {
            assert!(
                std::time::Instant::now() < deadline,
                "the device never linked"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let stopping = std::time::Instant::now();
        server.stop();
        assert!(stopping.elapsed() < DRAIN, "stop waited out the drain");
        assert!(!linked(), "the link outlived the server");
        // The device is told, so it links again once the server is back.
        let closed = loop {
            match socket.read() {
                Ok(tungstenite::Message::Close(frame)) => break frame,
                Ok(_) => continue,
                Err(e) => panic!("no close frame: {e}"),
            }
        };
        assert_eq!(
            closed.map(|f| f.code),
            Some(tungstenite::protocol::frame::coding::CloseCode::Away)
        );

        let again = start();
        ping(again.addr());
        again.stop();
    }
}

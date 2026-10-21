//! kōan's push relay: forwards pushes from any kōan server to Apple, for
//! servers that do not hold the developer's key.
//!
//! The protocol is specified in `docs/reference/protocol/push-relay.md`. In
//! short: an app registers its device token and gets back a handle, the token
//! sealed under a key only the relay holds, which it gives its servers. A
//! server sends the relay a handle and a kind of push, plus for a
//! notification a blob encrypted to the app. The relay writes everything a
//! phone shows itself, so a hostile server can do no more than send koan's
//! own generic text, within quotas, to people who signed in to it.
//!
//! Every push is answered from memory: handles carry their device, quotas are
//! counters, and the revocation table is held whole. The disk is touched only
//! to revoke.

pub mod apns;
pub mod config;
pub mod handle;
mod metrics;
pub mod quota;
pub mod revocations;

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use crate::apns::{Apns, Delivery, Kind, MAX_BLOB};
use crate::config::{Config, secret};
use crate::handle::{Handles, MAX_LIFETIME_MS, MAX_TOKEN, MIN_TOKEN, TokenHash, TokenHasher};
use crate::metrics::{Metrics, Outcome, Route};
use crate::quota::{Clock, Global, Keyed, Rate, Refused};
use crate::revocations::{Revocations, RevokeError};

/// The largest request body: a push is a handle, a kind and a blob.
const MAX_BODY: usize = 8 * 1024;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// What a caller is told to wait when the relay, not the caller, is the
/// reason for refusing.
const BUSY_RETRY: Duration = Duration::from_secs(5);

/// What a caller is told to wait when Apple refuses the relay itself, which
/// takes an operator to fix.
const BROKEN_RETRY: Duration = Duration::from_secs(5 * 60);

const SWEEP_EVERY: Duration = Duration::from_secs(60);
const PRUNE_EVERY: Duration = Duration::from_secs(60 * 60);

const HOUR: Duration = Duration::from_secs(60 * 60);
const MINUTE: Duration = Duration::from_secs(60);

pub struct Relay {
    handles: Handles,
    hasher: TokenHasher,
    revocations: Revocations,
    apns: Apns,
    clock: Clock,
    handle_alerts: Keyed<HandleKey>,
    handle_wakes: Keyed<HandleKey>,
    device_alerts: Keyed<TokenHash>,
    device_wakes: Keyed<TokenHash>,
    ip_pushes: Keyed<u128>,
    ip_registrations: Keyed<u128>,
    ip_revocations: Keyed<u128>,
    global: Global,
    in_flight: Semaphore,
    max_in_flight: usize,
    client_ip_header: Option<HeaderName>,
    metrics: Metrics,
}

impl Relay {
    pub fn new(cfg: &Config) -> Result<Arc<Self>, String> {
        let keys = cfg
            .handles
            .keys
            .iter()
            .map(|(id, key)| {
                let id = id
                    .parse::<u8>()
                    .map_err(|_| format!("handle key id {id:?} is not 1 to 255"))?;
                Ok((id, secret(&format!("handles.keys.{id}"), key)?))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let lifetime_ms = cfg.handles.lifetime_days * 24 * 60 * 60 * 1000;
        if lifetime_ms == 0 || lifetime_ms > MAX_LIFETIME_MS {
            return Err("handles.lifetime_days must be 1 to 365".into());
        }
        let handles = Handles::new(&keys, cfg.handles.current, lifetime_ms)?;
        let hasher = TokenHasher::new(&secret("token_key", &cfg.token_key)?);
        let revocations = Revocations::open(
            &cfg.store,
            cfg.limits.max_revocations,
            unix_ms(),
            MAX_LIFETIME_MS,
        )?;
        let apns = Apns::new(&cfg.apns, &cfg.topic)?;
        let client_ip_header = cfg
            .client_ip_header
            .as_deref()
            .map(|h| HeaderName::try_from(h).map_err(|_| format!("{h:?} is not a header name")))
            .transpose()?;
        let l = &cfg.limits;
        Ok(Arc::new(Self {
            handles,
            hasher,
            revocations,
            apns,
            clock: Clock::new(),
            handle_alerts: Keyed::new(Rate::per(l.handle_alerts_per_hour, HOUR), l.max_tracked),
            handle_wakes: Keyed::new(Rate::per(l.handle_wakes_per_hour, HOUR), l.max_tracked),
            device_alerts: Keyed::new(Rate::per(l.device_alerts_per_hour, HOUR), l.max_tracked),
            device_wakes: Keyed::new(Rate::per(l.device_wakes_per_hour, HOUR), l.max_tracked),
            ip_pushes: Keyed::new(Rate::per(l.ip_pushes_per_minute, MINUTE), l.max_tracked),
            ip_registrations: Keyed::new(
                Rate::per(l.ip_registrations_per_minute, MINUTE),
                l.max_tracked,
            ),
            ip_revocations: Keyed::new(
                Rate::per(l.ip_revocations_per_minute, MINUTE),
                l.max_tracked,
            ),
            global: Global::new(Rate::per(
                l.global_pushes_per_second,
                Duration::from_secs(1),
            )),
            in_flight: Semaphore::new(cfg.apns.max_in_flight),
            max_in_flight: cfg.apns.max_in_flight,
            client_ip_header,
            metrics: Metrics::default(),
        }))
    }

    /// Who is calling, for per-address limits. IPv6 callers are counted by
    /// their /64, which is what one subscriber is usually given.
    fn caller(&self, headers: &HeaderMap, peer: SocketAddr) -> u128 {
        let ip = self
            .client_ip_header
            .as_ref()
            // The last address is the one the nearest proxy added, whether
            // it appended to the caller's header or added one of its own;
            // anything before it came from the caller.
            .and_then(|name| headers.get_all(name).iter().next_back())
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
            .unwrap_or(peer.ip());
        match ip {
            IpAddr::V4(v4) => u128::from(v4.to_ipv6_mapped()),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => u128::from(v4.to_ipv6_mapped()),
                None => u128::from(v6) & !((1u128 << 64) - 1),
            },
        }
    }

    fn sweep(&self) {
        let now = self.clock.now();
        self.handle_alerts.sweep(now);
        self.handle_wakes.sweep(now);
        self.device_alerts.sweep(now);
        self.device_wakes.sweep(now);
        self.ip_pushes.sweep(now);
        self.ip_registrations.sweep(now);
        self.ip_revocations.sweep(now);
    }
}

/// The public API.
pub fn router(relay: Arc<Relay>, max_requests: usize) -> Router {
    let routes = Router::new()
        .route("/v1/register", post(register))
        .route("/v1/revoke", post(revoke))
        .route("/v1/push", post(push))
        .with_state(relay)
        // Innermost so it is inside the timeout: a panic becomes a 500
        // rather than a dropped connection.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(MAX_BODY))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ));
    // Shed rather than queue: an overloaded relay tells the surplus to come
    // back instead of answering everyone slowly. The limit wraps the routes
    // as one service, so every request counts against it; a layer on a
    // router would wrap each route apart.
    let limited = tower::ServiceBuilder::new()
        .layer(axum::error_handling::HandleErrorLayer::new(
            |err: tower::BoxError| async move {
                if err.is::<tower::load_shed::error::Overloaded>() {
                    refuse(StatusCode::SERVICE_UNAVAILABLE, "busy", Some(BUSY_RETRY))
                } else {
                    refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal", None)
                }
            },
        ))
        .load_shed()
        .concurrency_limit(max_requests)
        .service(routes);
    // Outside the limit: a health check answered 503 under load gets a busy
    // relay restarted.
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .fallback_service(limited)
}

/// Metrics and a health check, for the cluster.
pub fn metrics_router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route(
            "/metrics",
            get(|State(relay): State<Arc<Relay>>| async move {
                let gauges = [
                    (
                        "koan_relay_apns_in_flight",
                        "Pushes waiting on Apple.",
                        (relay.max_in_flight - relay.in_flight.available_permits()) as u64,
                    ),
                    (
                        "koan_relay_revocations",
                        "Devices in the revocation table.",
                        relay.revocations.len() as u64,
                    ),
                    (
                        "koan_relay_tracked_devices",
                        "Devices with a notification count.",
                        relay.device_alerts.tracked() as u64,
                    ),
                    (
                        "koan_relay_tracked_callers",
                        "Addresses with a push count.",
                        relay.ip_pushes.tracked() as u64,
                    ),
                ];
                (
                    [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
                    relay.metrics.render(&gauges),
                )
            }),
        )
        .route("/healthz", get(|| async { "ok" }))
        .with_state(relay)
}

/// Serve the relay on `listener`, and its metrics on `metrics` if given,
/// until `shutdown` completes.
pub async fn serve(
    relay: Arc<Relay>,
    max_requests: usize,
    listener: TcpListener,
    metrics: Option<TcpListener>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let upkeep = tokio::spawn(upkeep(relay.clone()));
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let metrics = metrics.map(|listener| {
        let mut stopped = stopped.clone();
        let app = metrics_router(relay.clone());
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stopped.wait_for(|s| *s).await;
                })
                .await
        })
    });
    let app = router(relay, max_requests);
    let served = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;
        let _ = stop.send(true);
    })
    .await;
    upkeep.abort();
    if let Some(metrics) = metrics {
        let _ = metrics.await;
    }
    served.map_err(|e| e.to_string())
}

async fn upkeep(relay: Arc<Relay>) {
    let mut sweep = tokio::time::interval(SWEEP_EVERY);
    let mut prune = tokio::time::interval(PRUNE_EVERY);
    loop {
        tokio::select! {
            // A sweep walks every limiter: millions of entries at capacity,
            // which is no work for a runtime thread.
            _ = sweep.tick() => {
                let relay = relay.clone();
                let _ = tokio::task::spawn_blocking(move || relay.sweep()).await;
            }
            _ = prune.tick() => {
                let relay = relay.clone();
                let pruned = tokio::task::spawn_blocking(move || {
                    relay.revocations.prune(unix_ms(), MAX_LIFETIME_MS)
                })
                .await;
                if let Ok(Err(e)) = pruned {
                    log::error!("pruning revocations: {e}");
                }
            }
        }
    }
}

/// What the app sends to register or revoke.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenRequest {
    /// The device token, hex.
    token: String,
    #[serde(default)]
    sandbox: bool,
}

#[derive(Serialize)]
struct Registered {
    handle: String,
    /// Unix seconds after which the handle no longer opens.
    expires: u64,
}

/// A handle's own count, keyed by its nonce, which no two handles share.
type HandleKey = [u8; aws_lc_rs::aead::NONCE_LEN];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushRequest {
    handle: String,
    kind: Kind,
    /// For a notification: what the app's extension decrypts, base64url.
    #[serde(default)]
    blob: Option<String>,
}

impl Relay {
    /// Send a token a background push, to learn that Apple knows it. A
    /// handle is minted and a revocation recorded only for a real device,
    /// so neither can be had for a made-up token. The app is open when it
    /// registers or revokes, so the wake asks nothing of it.
    async fn prove(&self, token: Vec<u8>, sandbox: bool) -> Result<(), Refusal> {
        if !self.apns.serves(sandbox) {
            return Err(Refusal::Bad("this relay does not serve that gateway"));
        }
        self.global
            .check(self.clock.now())
            .map_err(|_| Refusal::Busy)?;
        let Ok(_permit) = self.in_flight.try_acquire() else {
            self.global.refund();
            return Err(Refusal::Busy);
        };
        let probe = handle::Device {
            token,
            sandbox,
            minted_ms: 0,
            id: [0; aws_lc_rs::aead::NONCE_LEN],
        };
        match self.apns.send(&probe, Kind::WakeNow, None).await {
            Delivery::Sent => Ok(()),
            Delivery::Gone => Err(Refusal::Bad("not a device token")),
            Delivery::Throttled => Err(Refusal::Limited(Duration::from_secs(60))),
            delivery => {
                self.global.refund();
                Err(unavailable(delivery))
            }
        }
    }

    /// Count a request's outcome and answer it.
    fn answer(&self, route: Route, answer: Result<Response, Refusal>) -> Response {
        match answer {
            Ok(response) => {
                self.metrics.count(route, Outcome::Ok);
                response
            }
            Err(refusal) => refusal.respond(self, route),
        }
    }
}

async fn register(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<TokenRequest>, JsonRejection>,
) -> Response {
    let answer = async {
        let caller = relay.caller(&headers, peer);
        relay
            .ip_registrations
            .check(caller, relay.clock.now())
            .map_err(limited)?;
        let Json(req) = body.map_err(|_| Refusal::Bad("malformed request"))?;
        let token = device_token(&req.token)?;
        let device = relay.hasher.hash(&token);
        relay.prove(token.clone(), req.sandbox).await?;
        let now = unix_ms();
        // A handle minted in the same millisecond as a revocation would be
        // revoked by it.
        let minted = relay
            .revocations
            .revoked_at(&device)
            .map_or(now, |at| now.max(at + 1));
        Ok(Json(Registered {
            handle: relay.handles.seal(&token, req.sandbox, minted),
            expires: (minted + relay.handles.lifetime_ms()) / 1000,
        })
        .into_response())
    }
    .await;
    relay.answer(Route::Register, answer)
}

async fn revoke(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<TokenRequest>, JsonRejection>,
) -> Response {
    let answer = async {
        let caller = relay.caller(&headers, peer);
        relay
            .ip_revocations
            .check(caller, relay.clock.now())
            .map_err(limited)?;
        let Json(req) = body.map_err(|_| Refusal::Bad("malformed request"))?;
        let token = device_token(&req.token)?;
        let device = relay.hasher.hash(&token);
        if !relay.revocations.has_room(&device) {
            return Err(Refusal::Busy);
        }
        relay.prove(token, req.sandbox).await?;
        let writer = relay.clone();
        let revoked =
            tokio::task::spawn_blocking(move || writer.revocations.revoke(device, unix_ms())).await;
        match revoked {
            Ok(Ok(())) => Ok(StatusCode::NO_CONTENT.into_response()),
            Ok(Err(RevokeError::Full)) => Err(Refusal::Busy),
            Ok(Err(RevokeError::Store(e))) => {
                log::error!("recording a revocation: {e}");
                Err(Refusal::Busy)
            }
            Err(e) => {
                log::error!("recording a revocation: {e}");
                Err(Refusal::Busy)
            }
        }
    }
    .await;
    relay.answer(Route::Revoke, answer)
}

async fn push(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<PushRequest>, JsonRejection>,
) -> Response {
    let answer = async {
        let caller = relay.caller(&headers, peer);
        let now = relay.clock.now();
        relay.ip_pushes.check(caller, now).map_err(limited)?;
        let Json(req) = body.map_err(|_| Refusal::Bad("malformed request"))?;
        if let Some(blob) = &req.blob {
            if !req.kind.is_alert() {
                return Err(Refusal::Bad("only a notification carries a blob"));
            }
            if blob.is_empty() || blob.len() > MAX_BLOB || !is_base64url(blob) {
                return Err(Refusal::Bad("the blob is not base64url of an allowed size"));
            }
        }
        let device = relay
            .handles
            .open(&req.handle, unix_ms())
            .map_err(|_| Refusal::Gone)?;
        let hash = relay.hasher.hash(&device.token);
        if relay.revocations.revoked(&hash, device.minted_ms) {
            return Err(Refusal::Gone);
        }

        // Each handle has its own allowance, so one server spending its own
        // cannot spend another's; the device's ceiling bounds them together.
        let (per_handle, per_device) = if req.kind.is_alert() {
            (&relay.handle_alerts, &relay.device_alerts)
        } else {
            (&relay.handle_wakes, &relay.device_wakes)
        };
        let handle_key = device.id;
        per_handle.check(handle_key, now).map_err(limited)?;
        if let Err(e) = per_device.check(hash, now) {
            per_handle.refund(&handle_key);
            return Err(limited(e));
        }
        // Spent only on a push that reaches Apple: one refused for the
        // relay's or Apple's sake hands its allowance back.
        let refund = |global: bool| {
            per_handle.refund(&handle_key);
            per_device.refund(&hash);
            if global {
                relay.global.refund();
            }
        };
        if relay.global.check(now).is_err() {
            refund(false);
            return Err(Refusal::Busy);
        }
        let Ok(_permit) = relay.in_flight.try_acquire() else {
            refund(true);
            return Err(Refusal::Busy);
        };
        match relay
            .apns
            .send(&device, req.kind, req.blob.as_deref())
            .await
        {
            Delivery::Sent => {
                relay.metrics.sent(req.kind);
                Ok(StatusCode::NO_CONTENT.into_response())
            }
            Delivery::Gone => Err(Refusal::Gone),
            Delivery::Throttled => Err(Refusal::Limited(Duration::from_secs(60))),
            delivery => {
                refund(true);
                Err(unavailable(delivery))
            }
        }
    }
    .await;
    relay.answer(Route::Push, answer)
}

/// The refusal for a push Apple could not take, logged without anything that
/// names the device.
fn unavailable(delivery: Delivery) -> Refusal {
    match delivery {
        Delivery::Rejected(reason) => {
            log::error!("apns refused the relay: {reason}");
            Refusal::Broken
        }
        Delivery::Unavailable(reason) => {
            log::warn!("apns: {reason}");
            Refusal::Busy
        }
        Delivery::Sent | Delivery::Gone | Delivery::Throttled => Refusal::Busy,
    }
}

enum Refusal {
    Bad(&'static str),
    /// The handle will never work: the caller should forget it.
    Gone,
    Limited(Duration),
    /// The relay or Apple cannot take it now.
    Busy,
    /// Apple refuses the relay itself until an operator fixes it.
    Broken,
}

impl Refusal {
    fn respond(self, relay: &Relay, route: Route) -> Response {
        let (outcome, response) = match self {
            Refusal::Bad(why) => (
                Outcome::BadRequest,
                refuse(StatusCode::BAD_REQUEST, why, None),
            ),
            Refusal::Gone => (Outcome::Gone, refuse(StatusCode::GONE, "gone", None)),
            Refusal::Limited(after) => (
                Outcome::Limited,
                refuse(StatusCode::TOO_MANY_REQUESTS, "rate limited", Some(after)),
            ),
            Refusal::Busy => (
                Outcome::Unavailable,
                refuse(StatusCode::SERVICE_UNAVAILABLE, "busy", Some(BUSY_RETRY)),
            ),
            Refusal::Broken => (
                Outcome::Unavailable,
                refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    Some(BROKEN_RETRY),
                ),
            ),
        };
        relay.metrics.count(route, outcome);
        response
    }
}

fn limited(refused: Refused) -> Refusal {
    match refused {
        Refused::Limited(after) => Refusal::Limited(after),
        Refused::Full => Refusal::Busy,
    }
}

fn refuse(status: StatusCode, error: &str, retry: Option<Duration>) -> Response {
    let mut response = (status, Json(json!({ "error": error }))).into_response();
    if let Some(after) = retry {
        let secs = after.as_secs() + u64::from(after.subsec_nanos() > 0);
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(secs.max(1)));
    }
    response
}

fn device_token(hex_token: &str) -> Result<Vec<u8>, Refusal> {
    if hex_token.len() > MAX_TOKEN * 2 {
        return Err(Refusal::Bad("not a device token"));
    }
    let token = hex::decode(hex_token).map_err(|_| Refusal::Bad("not a device token"))?;
    if token.len() < MIN_TOKEN {
        return Err(Refusal::Bad("not a device token"));
    }
    Ok(token)
}

fn is_base64url(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

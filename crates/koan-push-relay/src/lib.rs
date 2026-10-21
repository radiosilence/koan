//! kōan's push relay: forwards pushes from any kōan server to Apple, for
//! servers that do not hold the developer's key.
//!
//! The protocol is specified in `docs/reference/protocol/push-relay.md`. In
//! short: an app registers its device token and gets back a handle, the token
//! sealed under a key only the relay holds, which it gives its servers. A
//! server sends the relay a handle and a kind of push, plus for a
//! notification a blob encrypted to the app. The relay writes everything
//! Apple is sent itself; what a phone shows beyond its generic text is the
//! app's notification extension's to decide, from a fixed template.
//!
//! Everything that reaches Apple is bounded with the developer's account in
//! mind: tokens are proven before they get a handle, proofs are rate limited
//! and stop when Apple starts refusing tokens, a token Apple calls gone is
//! retired, and a credential Apple refuses stops the relay sending until an
//! operator fixes it.
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

use axum::body::Bytes;
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
use crate::revocations::{Revocations, RevokeError, SLACK_MS};

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

/// How long the relay stops sending once Apple refuses the relay itself, and
/// how often it lets one push through meanwhile to learn whether that is
/// fixed.
const BREAKER_OPEN: Duration = Duration::from_secs(5 * 60);
const BREAKER_PROBE: Duration = Duration::from_secs(60);

/// IPv6 callers to registration and revocation are counted by /48, the
/// largest block one site is commonly given; pushes by /64.
const PROOF_PREFIX: u32 = 48;
const PUSH_PREFIX: u32 = 64;

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
    device_revocations: Keyed<TokenHash>,
    ip_pushes: Keyed<u128>,
    ip_registrations: Keyed<u128>,
    ip_revocations: Keyed<u128>,
    global: Global,
    proofs: Global,
    bad_tokens: Global,
    breaker: Breaker,
    in_flight: Semaphore,
    max_in_flight: usize,
    client_ip_header: Option<HeaderName>,
    client_ip_hops: usize,
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
        let lifetime_ms = cfg
            .handles
            .lifetime_days
            .checked_mul(24 * 60 * 60 * 1000)
            .filter(|ms| (1..=MAX_LIFETIME_MS).contains(ms))
            .ok_or("handles.lifetime_days must be 1 to 365")?;
        if cfg.client_ip_hops == 0 {
            return Err("client_ip_hops must be at least 1".into());
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
            device_revocations: Keyed::new(
                Rate::per(l.device_revocations_per_hour, HOUR),
                l.max_tracked,
            ),
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
            proofs: Global::new(Rate::per(l.proofs_per_second, Duration::from_secs(1))),
            bad_tokens: Global::new(Rate::per(l.bad_tokens_per_hour, HOUR)),
            breaker: Breaker::default(),
            in_flight: Semaphore::new(cfg.apns.max_in_flight),
            max_in_flight: cfg.apns.max_in_flight,
            client_ip_header,
            client_ip_hops: cfg.client_ip_hops,
            metrics: Metrics::default(),
        }))
    }

    /// Who is calling, for per-address limits: an IPv4 address, or an IPv6
    /// address cut to `prefix` bits, since one subscriber holds a block.
    fn caller(&self, headers: &HeaderMap, peer: SocketAddr, prefix: u32) -> u128 {
        // The proxies in front append, as a line of their own or to the
        // caller's: counting `client_ip_hops` back from the end passes over
        // them, and anything further back came from the caller.
        let ip = self
            .client_ip_header
            .as_ref()
            .and_then(|name| {
                let values: Vec<&str> = headers
                    .get_all(name)
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .collect();
                values
                    .join(",")
                    .rsplit(',')
                    .nth(self.client_ip_hops - 1)
                    .and_then(|v| v.trim().parse::<IpAddr>().ok())
            })
            .unwrap_or(peer.ip());
        match ip {
            IpAddr::V4(v4) => u128::from(v4.to_ipv6_mapped()),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => u128::from(v4.to_ipv6_mapped()),
                None => u128::from(v6) & (u128::MAX << (128 - prefix)),
            },
        }
    }

    fn sweep(&self) {
        let now = self.clock.now();
        self.handle_alerts.sweep(now);
        self.handle_wakes.sweep(now);
        self.device_alerts.sweep(now);
        self.device_wakes.sweep(now);
        self.device_revocations.sweep(now);
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
    /// Send to Apple, unless Apple has lately refused the relay itself:
    /// then nothing goes but one push a minute, to learn whether that has
    /// been fixed, and the rest are refused here.
    async fn send(&self, device: &handle::Device, kind: Kind, blob: Option<&str>) -> Delivery {
        let now = self.clock.now();
        if !self.breaker.allows(now) {
            return Delivery::Rejected(String::new());
        }
        let delivery = self.apns.send(device, kind, blob).await;
        match &delivery {
            Delivery::Rejected(_) => self.breaker.trip(self.clock.now()),
            Delivery::Unavailable(_) => {}
            Delivery::Sent | Delivery::Gone | Delivery::Throttled => self.breaker.reset(),
        }
        delivery
    }

    /// Send a token a background push, to learn that Apple knows it. A
    /// handle is minted and a revocation recorded only for a real device,
    /// so neither can be had for a made-up token. The app is open when it
    /// registers or revokes, so the wake asks nothing of it.
    ///
    /// Proofs are where made-up tokens reach Apple, so they have limits of
    /// their own: a rate for everyone, and a count of tokens Apple refused,
    /// past which no proof is sent until it falls back. The device's own
    /// allowance (`per_device`) is looked at first and spent only once Apple
    /// has accepted the token, so a made-up token is never tracked.
    async fn prove(
        &self,
        token: Vec<u8>,
        sandbox: bool,
        per_device: &Keyed<TokenHash>,
    ) -> Result<(), Refusal> {
        if !self.apns.serves(sandbox) {
            return Err(Refusal::Bad("this relay does not serve that gateway"));
        }
        let device = self.hasher.hash(&token);
        let now = self.clock.now();
        per_device.admits(&device, now).map_err(limited)?;
        if !self.bad_tokens.admits(now) {
            return Err(Refusal::Busy);
        }
        self.proofs.check(now).map_err(|_| Refusal::Busy)?;
        let mut charge = Charge::new(self);
        self.global.check(now).map_err(|_| Refusal::Busy)?;
        charge.global = true;
        let Ok(_permit) = self.in_flight.try_acquire() else {
            return Err(Refusal::Busy);
        };
        let probe = handle::Device {
            token,
            sandbox,
            minted_ms: 0,
            id: [0; aws_lc_rs::aead::NONCE_LEN],
        };
        match self.send(&probe, Kind::WakeNow, None).await {
            Delivery::Sent => {
                charge.spent = true;
                // Spent now that the token is known real. Two proofs racing
                // past the look above may both land; the next is refused.
                let _ = per_device.check(device, self.clock.now());
                Ok(())
            }
            Delivery::Gone => {
                charge.spent = true;
                self.bad_tokens.charge(self.clock.now());
                Err(Refusal::Bad("not a device token"))
            }
            Delivery::Throttled => {
                charge.spent = true;
                Err(Refusal::Limited(Duration::from_secs(60)))
            }
            delivery => Err(unavailable(delivery)),
        }
    }

    /// Retire every handle for a device Apple says is gone, so no server
    /// can keep sending to it. Off the request: the answer is already known.
    fn retire(self: &Arc<Self>, device: TokenHash) {
        let relay = self.clone();
        tokio::task::spawn_blocking(move || match relay.revocations.revoke(device, unix_ms()) {
            Ok(()) | Err(RevokeError::Full) => {}
            Err(RevokeError::Store(e)) => log::error!("retiring a gone token: {e}"),
        });
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
    body: Bytes,
) -> Response {
    let answer = async {
        let caller = relay.caller(&headers, peer, PROOF_PREFIX);
        relay
            .ip_registrations
            .check(caller, relay.clock.now())
            .map_err(limited)?;
        let req: TokenRequest = parse(&body)?;
        let token = device_token(&req.token)?;
        let device = relay.hasher.hash(&token);
        relay
            .prove(token.clone(), req.sandbox, &relay.device_wakes)
            .await?;
        let now = unix_ms();
        // Past the reach of the device's last revocation, which would
        // otherwise retire this handle too.
        let minted = relay
            .revocations
            .revoked_at(&device)
            .map_or(now, |at| now.max(at + SLACK_MS + 1));
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
    body: Bytes,
) -> Response {
    let answer = async {
        let caller = relay.caller(&headers, peer, PROOF_PREFIX);
        relay
            .ip_revocations
            .check(caller, relay.clock.now())
            .map_err(limited)?;
        let req: TokenRequest = parse(&body)?;
        let token = device_token(&req.token)?;
        let device = relay.hasher.hash(&token);
        if !relay.revocations.has_room(&device) {
            return Err(Refusal::Busy);
        }
        relay
            .prove(token, req.sandbox, &relay.device_revocations)
            .await?;
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
    body: Bytes,
) -> Response {
    let answer = async {
        let caller = relay.caller(&headers, peer, PUSH_PREFIX);
        let now = relay.clock.now();
        relay.ip_pushes.check(caller, now).map_err(limited)?;
        let req: PushRequest = parse(&body)?;
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
        let mut charge = Charge::new(&relay);
        per_handle.check(handle_key, now).map_err(limited)?;
        charge.handle = Some((per_handle, handle_key));
        per_device.check(hash, now).map_err(limited)?;
        charge.device = Some((per_device, hash));
        relay.global.check(now).map_err(|_| Refusal::Busy)?;
        charge.global = true;
        let Ok(_permit) = relay.in_flight.try_acquire() else {
            return Err(Refusal::Busy);
        };
        let delivery = relay.send(&device, req.kind, req.blob.as_deref()).await;
        if matches!(
            delivery,
            Delivery::Sent | Delivery::Gone | Delivery::Throttled
        ) {
            charge.spent = true;
        }
        match delivery {
            Delivery::Sent => {
                relay.metrics.sent(req.kind);
                Ok(StatusCode::NO_CONTENT.into_response())
            }
            Delivery::Gone => {
                relay.retire(hash);
                Err(Refusal::Gone)
            }
            Delivery::Throttled => Err(Refusal::Limited(Duration::from_secs(60))),
            delivery => Err(unavailable(delivery)),
        }
    }
    .await;
    relay.answer(Route::Push, answer)
}

/// Allowances taken for one push to Apple, handed back when dropped unless
/// the push reached Apple: one refused for the relay's or Apple's sake, or
/// abandoned when its request timed out, costs its sender nothing.
struct Charge<'a> {
    relay: &'a Relay,
    handle: Option<(&'a Keyed<HandleKey>, HandleKey)>,
    device: Option<(&'a Keyed<TokenHash>, TokenHash)>,
    global: bool,
    spent: bool,
}

impl<'a> Charge<'a> {
    fn new(relay: &'a Relay) -> Self {
        Self {
            relay,
            handle: None,
            device: None,
            global: false,
            spent: false,
        }
    }
}

impl Drop for Charge<'_> {
    fn drop(&mut self) {
        if self.spent {
            return;
        }
        if let Some((limit, key)) = &self.handle {
            limit.refund(key);
        }
        if let Some((limit, key)) = &self.device {
            limit.refund(key);
        }
        if self.global {
            self.relay.global.refund();
        }
    }
}

/// The refusal for a push Apple could not take, logged without anything that
/// names the device.
fn unavailable(delivery: Delivery) -> Refusal {
    match delivery {
        // Empty when the breaker refused it here, which was logged when it
        // tripped.
        Delivery::Rejected(reason) => {
            if !reason.is_empty() {
                log::error!("apns refused the relay: {reason}");
            }
            Refusal::Broken
        }
        Delivery::Unavailable(reason) => {
            log::warn!("apns: {reason}");
            Refusal::Busy
        }
        Delivery::Sent | Delivery::Gone | Delivery::Throttled => Refusal::Busy,
    }
}

/// Open while Apple refuses the relay itself: a wrong key, team, topic or
/// environment. Sending on with a credential Apple has refused is what gets
/// a key looked at, so the relay stops, and probes now and then.
#[derive(Default)]
struct Breaker {
    /// Until when it stays open, in `Clock` time; zero when closed.
    open_until: std::sync::atomic::AtomicU64,
    /// When the next probe may go while open.
    next_probe: std::sync::atomic::AtomicU64,
}

impl Breaker {
    fn allows(&self, now: u64) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        if now >= self.open_until.load(Relaxed) {
            return true;
        }
        let next = self.next_probe.load(Relaxed);
        now >= next
            && self
                .next_probe
                .compare_exchange(
                    next,
                    now + BREAKER_PROBE.as_nanos() as u64,
                    Relaxed,
                    Relaxed,
                )
                .is_ok()
    }

    fn trip(&self, now: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        self.open_until
            .store(now + BREAKER_OPEN.as_nanos() as u64, Relaxed);
        self.next_probe
            .store(now + BREAKER_PROBE.as_nanos() as u64, Relaxed);
    }

    fn reset(&self) {
        self.open_until
            .store(0, std::sync::atomic::Ordering::Relaxed);
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

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Refusal> {
    serde_json::from_slice(body).map_err(|_| Refusal::Bad("malformed request"))
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

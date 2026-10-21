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
//! and stop for an address whose tokens Apple refuses, revoking asks nothing
//! of Apple, a token Apple calls gone is retired, and a credential Apple
//! refuses stops its gateway sending until a probe shows it is fixed.
//!
//! Every push is answered from memory: handles carry their device, quotas are
//! counters, and the revocation table is held whole. The disk is touched only
//! to revoke.

pub mod apns;
pub mod config;
mod handle;
mod metrics;
mod quota;
mod revocations;

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::apns::{Apns, Delivery, Kind, MAX_BLOB, Target};
use crate::config::{Config, secret};
use crate::handle::{
    Handles, Invalid, MAX_LIFETIME_MS, MAX_TOKEN, MIN_TOKEN, SLOT_LEN, Slot, TokenHash, TokenHasher,
};
use crate::metrics::{Metrics, Outcome, Route};
use crate::quota::{Clock, Global, Keyed, Rate, Refused};
use crate::revocations::{Revocations, RevokeError, SLACK_MS};

/// The largest request body: a push is a handle, a kind and a blob.
const MAX_BODY: usize = 8 * 1024;

/// How long a body may take to arrive. Short: every caller sends a few
/// hundred bytes, and a body held back holds a connection.
const BODY_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Proven tokens Apple refuses, across devices, before the relay takes it
/// that its own topic or gateway is wrong and stops the gateway. One alone
/// is a single stale token, retired like a gone one.
const REFUSED_PROVEN: u32 = 5;
const REFUSED_PROVEN_WINDOW: Duration = Duration::from_secs(10 * 60);

/// IPv6 callers to registration and revocation are counted by /48, the
/// largest block one site is commonly given, so a site cannot spread made-up
/// tokens across its addresses; the price is that neighbours in one /48
/// share an allowance. Pushes, which servers send, are counted by /64.
const PROOF_PREFIX: u32 = 48;
const PUSH_PREFIX: u32 = 64;

/// One server's allowance for one device: the device, and the slot the app
/// gave that server, which renewals keep.
type SlotKey = (TokenHash, Slot);

pub struct Relay {
    handles: Handles,
    hasher: TokenHasher,
    revocations: Revocations,
    apns: Apns,
    clock: Clock,
    handle_alerts: Keyed<SlotKey>,
    handle_wakes: Keyed<SlotKey>,
    device_alerts: Keyed<TokenHash>,
    device_wakes: Keyed<TokenHash>,
    device_revocations: Keyed<TokenHash>,
    ip_pushes: Keyed<u128>,
    ip_registrations: Keyed<u128>,
    ip_revocations: Keyed<u128>,
    ip_bad_tokens: Keyed<u128>,
    global: Global,
    proofs: Global,
    bad_tokens: Global,
    refused_proven: Global,
    in_flight: Arc<Semaphore>,
    max_in_flight: usize,
    requests: Semaphore,
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
        let l = &cfg.limits;
        let positive = [
            ("handle_alerts_per_hour", l.handle_alerts_per_hour),
            ("handle_wakes_per_hour", l.handle_wakes_per_hour),
            ("device_alerts_per_hour", l.device_alerts_per_hour),
            ("device_wakes_per_hour", l.device_wakes_per_hour),
            ("device_revocations_per_hour", l.device_revocations_per_hour),
            ("ip_pushes_per_minute", l.ip_pushes_per_minute),
            ("ip_registrations_per_minute", l.ip_registrations_per_minute),
            ("ip_revocations_per_minute", l.ip_revocations_per_minute),
            ("ip_bad_tokens_per_hour", l.ip_bad_tokens_per_hour),
            ("bad_tokens_per_hour", l.bad_tokens_per_hour),
            ("proofs_per_second", l.proofs_per_second),
            ("global_pushes_per_second", l.global_pushes_per_second),
        ];
        if let Some((name, _)) = positive.iter().find(|(_, n)| *n == 0) {
            return Err(format!("limits.{name} must be at least 1"));
        }
        let sized = [
            ("limits.max_tracked", l.max_tracked),
            ("limits.max_revocations", l.max_revocations),
            ("limits.max_requests", l.max_requests),
            ("apns.max_in_flight", cfg.apns.max_in_flight),
        ];
        if let Some((name, _)) = sized.iter().find(|(_, n)| !(1..=1 << 30).contains(n)) {
            return Err(format!("{name} must be 1 to 2^30"));
        }
        if cfg.client_ip_hops == 0 {
            return Err("client_ip_hops must be at least 1".into());
        }
        let handles = Handles::new(&keys, cfg.handles.current, lifetime_ms)?;
        let hasher = TokenHasher::new(&secret("token_key", &cfg.token_key)?);
        let revocations =
            Revocations::open(&cfg.store, l.max_revocations, unix_ms(), MAX_LIFETIME_MS)?;
        let apns = Apns::new(&cfg.apns, &cfg.topic)?;
        let client_ip_header = cfg
            .client_ip_header
            .as_deref()
            .map(|h| HeaderName::try_from(h).map_err(|_| format!("{h:?} is not a header name")))
            .transpose()?;
        let second = Duration::from_secs(1);
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
            ip_bad_tokens: Keyed::new(Rate::per(l.ip_bad_tokens_per_hour, HOUR), l.max_tracked),
            global: Global::new(Rate::per(l.global_pushes_per_second, second)),
            proofs: Global::new(Rate::per(l.proofs_per_second, second)),
            bad_tokens: Global::new(Rate::per(l.bad_tokens_per_hour, HOUR)),
            refused_proven: Global::new(Rate::per(REFUSED_PROVEN, REFUSED_PROVEN_WINDOW)),
            in_flight: Arc::new(Semaphore::new(cfg.apns.max_in_flight)),
            max_in_flight: cfg.apns.max_in_flight,
            requests: Semaphore::new(l.max_requests),
            client_ip_header,
            client_ip_hops: cfg.client_ip_hops,
            metrics: Metrics::default(),
        }))
    }

    /// Who is calling, for per-address limits: an IPv4 address, or an IPv6
    /// address cut to `prefix` bits, since one subscriber holds a block.
    ///
    /// Behind a proxy (`client_ip_header`), a request without a readable
    /// address there is refused: counting it as the proxy's own address
    /// would put every such caller in one bucket, where one could spend
    /// everyone's allowance.
    fn caller(&self, headers: &HeaderMap, peer: SocketAddr, prefix: u32) -> Result<u128, Refusal> {
        let ip = match &self.client_ip_header {
            None => peer.ip(),
            Some(name) => {
                // The proxies in front append, as a line of their own or to
                // the caller's: counting `client_ip_hops` back from the end
                // passes over them, and anything further back came from the
                // caller.
                let values: Vec<&str> = headers
                    .get_all(name)
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .collect();
                let found = values
                    .join(",")
                    .rsplit(',')
                    .nth(self.client_ip_hops - 1)
                    .and_then(|v| v.trim().parse::<IpAddr>().ok());
                match found {
                    Some(ip) => ip,
                    None => {
                        static SAID: std::sync::atomic::AtomicBool =
                            std::sync::atomic::AtomicBool::new(false);
                        if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            log::error!(
                                "a request came without a readable caller in {name}: is the proxy setting it?"
                            );
                        }
                        return Err(Refusal::Bad("no caller address"));
                    }
                }
            }
        };
        Ok(match ip {
            IpAddr::V4(v4) => u128::from(v4.to_ipv6_mapped()),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => u128::from(v4.to_ipv6_mapped()),
                None => u128::from(v6) & (u128::MAX << (128 - prefix)),
            },
        })
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
        self.ip_bad_tokens.sweep(now);
    }

    fn take(&self, allowance: Allowance, now: u64) -> Result<(), Refusal> {
        match allowance {
            Allowance::HandleAlerts(k) => self.handle_alerts.check(k, now).map_err(limited),
            Allowance::HandleWakes(k) => self.handle_wakes.check(k, now).map_err(limited),
            Allowance::DeviceAlerts(k) => self.device_alerts.check(k, now).map_err(limited),
            Allowance::DeviceWakes(k) => self.device_wakes.check(k, now).map_err(limited),
            Allowance::Global => self.global.check(now).map_err(|_| Refusal::Busy),
            Allowance::Proofs => self.proofs.check(now).map_err(|_| Refusal::Busy),
            Allowance::RefusedFrom(k) => self.ip_bad_tokens.check(k, now).map_err(limited),
            Allowance::Refused => self.bad_tokens.check(now).map_err(|_| Refusal::Busy),
        }
    }

    fn give_back(&self, allowance: Allowance) {
        match allowance {
            Allowance::HandleAlerts(k) => self.handle_alerts.refund(&k),
            Allowance::HandleWakes(k) => self.handle_wakes.refund(&k),
            Allowance::DeviceAlerts(k) => self.device_alerts.refund(&k),
            Allowance::DeviceWakes(k) => self.device_wakes.refund(&k),
            Allowance::Global => self.global.refund(),
            Allowance::Proofs => self.proofs.refund(),
            Allowance::RefusedFrom(k) => self.ip_bad_tokens.refund(&k),
            Allowance::Refused => self.bad_tokens.refund(),
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

/// An allowance a push to Apple is counted against.
#[derive(Clone, Copy)]
enum Allowance {
    HandleAlerts(SlotKey),
    HandleWakes(SlotKey),
    DeviceAlerts(TokenHash),
    DeviceWakes(TokenHash),
    Global,
    Proofs,
    /// A proof's room in its caller's count of refused tokens, and in
    /// everyone's: taken before it is sent, so racing proofs cannot overrun
    /// either, and handed back if Apple accepts the token.
    RefusedFrom(u128),
    Refused,
}

/// Allowances taken for one push to Apple, handed back when dropped unless
/// the push reached Apple: one refused for the relay's or Apple's sake costs
/// its sender nothing. Owned, so it can go with the push into the task that
/// sends it.
struct Charge {
    relay: Arc<Relay>,
    taken: Vec<Allowance>,
    spent: bool,
}

impl Charge {
    /// Hand back the room taken in the counts of refused tokens: the token
    /// was real.
    fn not_refused(&mut self) {
        let relay = &self.relay;
        self.taken.retain(|a| match a {
            Allowance::RefusedFrom(_) | Allowance::Refused => {
                relay.give_back(*a);
                false
            }
            _ => true,
        });
    }

    fn new(relay: &Arc<Relay>) -> Self {
        Self {
            relay: relay.clone(),
            taken: Vec::with_capacity(4),
            spent: false,
        }
    }

    fn take(&mut self, allowance: Allowance, now: u64) -> Result<(), Refusal> {
        self.relay.take(allowance, now)?;
        self.taken.push(allowance);
        Ok(())
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        if !self.spent {
            for allowance in &self.taken {
                self.relay.give_back(*allowance);
            }
        }
    }
}

/// One push on its way to Apple.
struct Job {
    token: Vec<u8>,
    sandbox: bool,
    slot: Option<Slot>,
    kind: Kind,
    blob: Option<String>,
    device: TokenHash,
    /// Whether this is the relay's own proof of a token.
    proof: bool,
}

impl Relay {
    /// Send to Apple from a task of its own, which settles the allowances,
    /// and does whatever Apple's answer calls for, whether or not the caller
    /// is still there: a sender that hangs up once its push is on the way
    /// still pays for it, and a token Apple refuses is still counted.
    async fn deliver(
        self: &Arc<Self>,
        charge: Charge,
        permit: OwnedSemaphorePermit,
        job: Job,
    ) -> Delivery {
        let relay = self.clone();
        let task = tokio::spawn(async move {
            let _permit = permit;
            let mut charge = charge;
            let to = Target {
                token: &job.token,
                sandbox: job.sandbox,
                slot: job.slot.as_ref(),
            };
            let delivery = relay.apns.send(&to, job.kind, job.blob.as_deref()).await;
            let now = relay.clock.now();
            match (&delivery, job.proof) {
                (Delivery::Sent, false) => {
                    charge.spent = true;
                    relay.metrics.sent(job.kind);
                }
                (Delivery::Sent, true) => {
                    charge.not_refused();
                    charge.spent = true;
                    relay.metrics.proof(true);
                }
                (Delivery::Throttled, _) => {
                    charge.not_refused();
                    charge.spent = true;
                }
                // A device the app has left, or a proven token Apple now
                // refuses: no server may send to it again.
                (Delivery::Unregistered | Delivery::BadToken, false) => {
                    charge.spent = true;
                    let writer = relay.clone();
                    let device = job.device;
                    let retired = tokio::task::spawn_blocking(move || {
                        writer.revocations.revoke(device, unix_ms())
                    })
                    .await;
                    if let Ok(Err(RevokeError::Store(e))) = retired {
                        log::error!("retiring a gone token: {e}");
                    }
                    // Several proven tokens refused in a short while say the
                    // relay's topic or gateway has changed under them.
                    if delivery == Delivery::BadToken {
                        relay.refused_proven.charge(now);
                        if !relay.refused_proven.admits(now) {
                            log::error!(
                                "apns refuses proven tokens: is the topic or gateway right?"
                            );
                            relay.apns.trip(job.sandbox);
                        }
                    }
                }
                // A proof Apple refused: made up, or an app long gone, or
                // refused for a reason the relay does not know. It stays
                // counted against its caller and against everyone.
                (Delivery::Unregistered | Delivery::BadToken | Delivery::Refused(_), true) => {
                    charge.spent = true;
                    relay.metrics.proof(false);
                }
                (Delivery::Refused(reason), false) => {
                    charge.spent = true;
                    log::error!("apns refused a push: {reason}");
                }
                (Delivery::Rejected(reason), _) => {
                    log::error!("apns refused the relay: {reason}");
                }
                (Delivery::Unavailable(reason), _) => log::warn!("apns: {reason}"),
                (Delivery::Unserved | Delivery::Held, _) => {}
            }
            delivery
        });
        task.await
            .unwrap_or_else(|e| Delivery::Unavailable(format!("delivery task: {e}")))
    }

    /// Send a token a background push, to learn that Apple knows it, before
    /// minting it a handle, so none can be had for a made-up token. The app
    /// is open when it registers, so the wake asks nothing of it.
    ///
    /// Proofs are where made-up tokens reach Apple, so they have limits of
    /// their own: a rate for everyone, and room in two counts of tokens
    /// Apple refused, one for the address asking and one for everyone,
    /// taken before the proof is sent and handed back if Apple accepts it.
    /// A proof is a wake like any other to the device it is for.
    async fn prove(
        self: &Arc<Self>,
        token: Vec<u8>,
        sandbox: bool,
        prover: u128,
    ) -> Result<TokenHash, Refusal> {
        if !self.apns.serves(sandbox) {
            return Err(Refusal::Bad("this relay does not serve that gateway"));
        }
        let device = self.hasher.hash(&token);
        let now = self.clock.now();
        let mut charge = Charge::new(self);
        charge.take(Allowance::RefusedFrom(prover), now)?;
        charge.take(Allowance::Refused, now)?;
        charge.take(Allowance::Proofs, now)?;
        charge.take(Allowance::DeviceWakes(device), now)?;
        charge.take(Allowance::Global, now)?;
        let permit = self
            .in_flight
            .clone()
            .try_acquire_owned()
            .map_err(|_| Refusal::Busy)?;
        let job = Job {
            token,
            sandbox,
            slot: None,
            kind: Kind::WakeNow,
            blob: None,
            device,
            proof: true,
        };
        match self.deliver(charge, permit, job).await {
            Delivery::Sent => Ok(device),
            Delivery::Unregistered | Delivery::BadToken | Delivery::Refused(_) => {
                Err(Refusal::Bad("not a device token"))
            }
            Delivery::Unserved => Err(Refusal::Bad("this relay does not serve that gateway")),
            Delivery::Throttled => Err(Refusal::Limited(Duration::from_secs(60))),
            Delivery::Unavailable(_) => Err(Refusal::Busy),
            Delivery::Rejected(_) | Delivery::Held => Err(Refusal::Broken),
        }
    }
}

/// The public API.
pub fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/v1/register", post(register))
        .route("/v1/revoke", post(revoke))
        .route("/v1/push", post(push))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(relay)
        // Innermost so it is inside the timeout: a panic becomes a 500
        // rather than a dropped connection.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(MAX_BODY))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
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
                        "koan_relay_production_held",
                        "1 while Apple refuses the production key and the relay has stopped sending.",
                        u64::from(relay.apns.held(false)),
                    ),
                    (
                        "koan_relay_sandbox_held",
                        "1 while Apple refuses the sandbox key and the relay has stopped sending.",
                        u64::from(relay.apns.held(true)),
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
    let app = router(relay.clone());
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
    let _ = tokio::task::spawn_blocking(move || relay.revocations.save_minted()).await;
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
                let saved = tokio::task::spawn_blocking(move || {
                    relay.sweep();
                    relay.revocations.save_minted()
                })
                .await;
                if let Ok(Err(e)) = saved {
                    log::error!("saving the latest mint time: {e}");
                }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterRequest {
    /// The device token, hex.
    token: String,
    #[serde(default)]
    sandbox: bool,
    /// Sixteen bytes, hex, that the app chose for the server this handle is
    /// for, and keeps for it across renewals.
    slot: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeRequest {
    /// A revoker from any registration of the device's that has not expired.
    revoker: String,
}

#[derive(Serialize)]
struct Registered {
    handle: String,
    /// The app's own key to revoke the device's handles. Never given to a
    /// server.
    revoker: String,
    /// Unix seconds after which the handle and revoker no longer open.
    expires: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushRequest {
    handle: String,
    kind: Kind,
    /// For a notification: what the app's extension decrypts, base64url.
    #[serde(default)]
    blob: Option<String>,
}

async fn register(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let answer = async {
        json_only(&headers)?;
        let caller = relay.caller(&headers, peer, PROOF_PREFIX)?;
        relay
            .ip_registrations
            .check(caller, relay.clock.now())
            .map_err(limited)?;
        let req: RegisterRequest = read(body).await?;
        let _working = relay.requests.try_acquire().map_err(|_| Refusal::Busy)?;
        let token = device_token(&req.token)?;
        let slot = slot(&req.slot)?;
        let device = relay.prove(token.clone(), req.sandbox, caller).await?;
        let now = unix_ms();
        // Past the reach of the device's last revocation, which would
        // otherwise retire this handle too. Only the clock's own reading
        // moves the relay's latest mint time, so registering cannot push it
        // ahead.
        relay.revocations.minted(now);
        let minted = relay
            .revocations
            .revoked_at(&device)
            .map_or(now, |at| now.max(at + SLACK_MS + 1));
        let seal = |revoker| {
            relay
                .handles
                .seal(&token, &slot, req.sandbox, minted, revoker)
        };
        Ok(Json(Registered {
            handle: seal(false),
            revoker: seal(true),
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
    body: Body,
) -> Response {
    let answer = async {
        json_only(&headers)?;
        let caller = relay.caller(&headers, peer, PROOF_PREFIX)?;
        relay
            .ip_revocations
            .check(caller, relay.clock.now())
            .map_err(limited)?;
        let req: RevokeRequest = read(body).await?;
        let _working = relay.requests.try_acquire().map_err(|_| Refusal::Busy)?;
        // Revoking asks nothing of Apple: a revoker is proof enough, since
        // the relay minted it only for a token Apple accepted, and only the
        // app holds one.
        let revoker = relay
            .handles
            .open(&req.revoker, unix_ms())
            .map_err(|e| match e {
                Invalid::Future => Refusal::Busy,
                _ => Refusal::Gone,
            })?;
        if !revoker.revoker {
            return Err(Refusal::Bad("not a revoker"));
        }
        let device = relay.hasher.hash(&revoker.token);
        if relay.revocations.revoked(&device, revoker.minted_ms) {
            return Err(Refusal::Gone);
        }
        if !relay.revocations.has_room(&device) {
            return Err(Refusal::Busy);
        }
        relay
            .device_revocations
            .check(device, relay.clock.now())
            .map_err(limited)?;
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
    body: Body,
) -> Response {
    let answer = async {
        json_only(&headers)?;
        let caller = relay.caller(&headers, peer, PUSH_PREFIX)?;
        relay
            .ip_pushes
            .check(caller, relay.clock.now())
            .map_err(limited)?;
        let req: PushRequest = read(body).await?;
        let _working = relay.requests.try_acquire().map_err(|_| Refusal::Busy)?;
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
            .map_err(|e| match e {
                Invalid::Future => Refusal::Busy,
                _ => Refusal::Gone,
            })?;
        if device.revoker {
            return Err(Refusal::Bad("a revoker is not a handle"));
        }
        let hash = relay.hasher.hash(&device.token);
        if relay.revocations.revoked(&hash, device.minted_ms) {
            return Err(Refusal::Gone);
        }

        // Each server has its own allowance for the device, so one spending
        // its own cannot spend another's; the device's ceiling bounds them
        // together.
        let key = (hash, device.slot);
        let (per_server, per_device) = if req.kind.is_alert() {
            (Allowance::HandleAlerts(key), Allowance::DeviceAlerts(hash))
        } else {
            (Allowance::HandleWakes(key), Allowance::DeviceWakes(hash))
        };
        let now = relay.clock.now();
        let mut charge = Charge::new(&relay);
        charge.take(per_server, now)?;
        charge.take(per_device, now)?;
        charge.take(Allowance::Global, now)?;
        let permit = relay
            .in_flight
            .clone()
            .try_acquire_owned()
            .map_err(|_| Refusal::Busy)?;
        let job = Job {
            token: device.token,
            sandbox: device.sandbox,
            slot: Some(device.slot),
            kind: req.kind,
            blob: req.blob,
            device: hash,
            proof: false,
        };
        match relay.deliver(charge, permit, job).await {
            Delivery::Sent => Ok(StatusCode::NO_CONTENT.into_response()),
            Delivery::Unregistered | Delivery::BadToken | Delivery::Unserved => Err(Refusal::Gone),
            Delivery::Throttled => Err(Refusal::Limited(Duration::from_secs(60))),
            Delivery::Unavailable(_) => Err(Refusal::Busy),
            Delivery::Refused(_) | Delivery::Rejected(_) | Delivery::Held => Err(Refusal::Broken),
        }
    }
    .await;
    relay.answer(Route::Push, answer)
}

enum Refusal {
    Bad(&'static str),
    /// Not JSON. Refused before anything else, so a web page cannot make a
    /// visitor's browser call the relay without a CORS preflight, which the
    /// relay never answers.
    NotJson,
    /// The body did not arrive in time, or was larger than any request.
    Body(StatusCode),
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
            Refusal::NotJson => (
                Outcome::BadRequest,
                refuse(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "requests are application/json",
                    None,
                ),
            ),
            Refusal::Body(status) => (Outcome::BadRequest, refuse(status, "body", None)),
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

fn json_only(headers: &HeaderMap) -> Result<(), Refusal> {
    let essence = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim);
    match essence {
        Some(t) if t.eq_ignore_ascii_case("application/json") => Ok(()),
        _ => Err(Refusal::NotJson),
    }
}

/// The body, within `BODY_TIMEOUT` and `MAX_BODY`, as `T`.
async fn read<T: serde::de::DeserializeOwned>(body: Body) -> Result<T, Refusal> {
    let bytes = tokio::time::timeout(BODY_TIMEOUT, axum::body::to_bytes(body, MAX_BODY))
        .await
        .map_err(|_| Refusal::Body(StatusCode::REQUEST_TIMEOUT))?
        .map_err(|_| Refusal::Body(StatusCode::PAYLOAD_TOO_LARGE))?;
    serde_json::from_slice(&bytes).map_err(|_| Refusal::Bad("malformed request"))
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

fn slot(hex_slot: &str) -> Result<Slot, Refusal> {
    let mut slot = [0; SLOT_LEN];
    hex::decode_to_slice(hex_slot, &mut slot).map_err(|_| Refusal::Bad("not a slot"))?;
    Ok(slot)
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

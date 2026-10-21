//! The relay end to end, against a stand-in for Apple's gateway.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use koan_push_relay::Relay;
use koan_push_relay::apns::{ALERT_BODY, ALERT_TITLE};
use koan_push_relay::config::{ApnsKey, Config};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
/// The slot the app gave the server a test pushes from.
const SLOT: &str = "00112233445566778899aabbccddeeff";

#[derive(Clone, Debug)]
struct Received {
    token: String,
    headers: HeaderMap,
    body: Value,
}

#[derive(Clone, Default)]
struct Gateway {
    received: Arc<Mutex<Vec<Received>>>,
    /// What the gateway answers: a status and a reason.
    answer: Arc<Mutex<Option<(u16, &'static str)>>>,
    /// How long the gateway takes to answer.
    delay: Arc<Mutex<std::time::Duration>>,
    /// Pushes that have reached the gateway, answered or not.
    arrived: Arc<std::sync::atomic::AtomicUsize>,
}

impl Gateway {
    fn received(&self) -> Vec<Received> {
        self.received.lock().clone()
    }

    fn answer(&self, status: u16, reason: &'static str) {
        *self.answer.lock() = Some((status, reason));
    }

    fn accept(&self) {
        *self.answer.lock() = None;
    }
}

async fn gateway_push(
    State(gw): State<Gateway>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: axum::Json<Value>,
) -> (StatusCode, axum::Json<Value>) {
    gw.arrived.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let delay = *gw.delay.lock();
    tokio::time::sleep(delay).await;
    gw.received.lock().push(Received {
        token,
        headers,
        body: body.0,
    });
    match *gw.answer.lock() {
        Some((status, reason)) => (
            StatusCode::from_u16(status).unwrap(),
            axum::Json(json!({ "reason": reason })),
        ),
        None => (StatusCode::OK, axum::Json(json!({}))),
    }
}

fn pem() -> String {
    let key = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
    let der = key.to_pkcs8v1().unwrap();
    let b64 = STANDARD.encode(der.as_ref());
    let lines: Vec<_> = b64
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        lines.join("\n")
    )
}

struct Harness {
    url: String,
    metrics: String,
    gateway: Gateway,
    http: reqwest::Client,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start(tune: impl FnOnce(&mut Config)) -> Self {
        let gateway = Gateway::default();
        let gw_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gw_addr = gw_listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route("/3/device/{token}", post(gateway_push))
            .with_state(gateway.clone());
        tokio::spawn(async move { axum::serve(gw_listener, app).await });

        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config {
            store: dir.path().join("relay.db"),
            token_key: STANDARD.encode([3u8; 32]),
            ..Config::default()
        };
        cfg.handles.keys = BTreeMap::from([("1".to_string(), STANDARD.encode([5u8; 32]))]);
        cfg.handles.current = 1;
        cfg.apns.team_id = "TEAM123456".into();
        cfg.apns.production = Some(ApnsKey {
            key_id: "KEY1234567".into(),
            key: Some(pem()),
            key_path: None,
        });
        cfg.apns.production_url = format!("http://{gw_addr}");
        cfg.apns.connections = 2;
        tune(&mut cfg);

        let relay = Relay::new(&cfg).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let metrics = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let metrics_addr = metrics.local_addr().unwrap();
        tokio::spawn(koan_push_relay::serve(
            relay,
            listener,
            Some(metrics),
            std::future::pending(),
        ));
        Self {
            url: format!("http://{addr}"),
            metrics: format!("http://{metrics_addr}/metrics"),
            gateway,
            http: reqwest::Client::new(),
            _dir: dir,
        }
    }

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.url))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn register(&self, token: &str) -> String {
        self.register_for(token, SLOT).await
    }

    /// A handle for the server the app gave `slot`.
    async fn register_for(&self, token: &str, slot: &str) -> String {
        self.register_with_revoker(token, slot).await.0
    }

    /// A handle, and the revoker only the app keeps.
    async fn register_with_revoker(&self, token: &str, slot: &str) -> (String, String) {
        let r = self
            .post("/v1/register", json!({ "token": token, "slot": slot }))
            .await;
        assert_eq!(r.status(), 200);
        let body: Value = r.json().await.unwrap();
        let field = |name: &str| body[name].as_str().unwrap().to_string();
        (field("handle"), field("revoker"))
    }

    async fn push(&self, handle: &str, kind: &str, blob: Option<&str>) -> reqwest::Response {
        let mut body = json!({ "handle": handle, "kind": kind });
        if let Some(blob) = blob {
            body["blob"] = json!(blob);
        }
        self.post("/v1/push", body).await
    }
}

#[tokio::test]
async fn a_wake_reaches_apple_as_a_bare_background_push() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    assert!(!handle.contains(TOKEN), "a handle never shows its token");

    let r = h.push(&handle, "wake", None).await;
    assert_eq!(r.status(), 204);

    // The first push is registration's proof that the token is real.
    let got = h.gateway.received()[1..].to_vec();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].token, TOKEN);
    assert_eq!(got[0].body, json!({ "aps": { "content-available": 1 } }));
    let header = |name: &str| got[0].headers[name].to_str().unwrap().to_string();
    assert_eq!(header("apns-topic"), "cc.blit.koan");
    assert_eq!(header("apns-push-type"), "background");
    assert_eq!(header("apns-priority"), "5");
    assert_eq!(header("apns-collapse-id"), "koan-wake");

    assert_eq!(h.push(&handle, "wake-now", None).await.status(), 204);
    let now = &h.gateway.received()[2];
    assert_eq!(now.headers["apns-collapse-id"], "koan-wake-now");
    assert!(header("authorization").starts_with("bearer "));
}

#[tokio::test]
async fn a_notification_shows_only_the_relays_text() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    assert_eq!(
        h.push(&handle, "notify", Some("c2VjcmV0")).await.status(),
        204
    );

    let body = &h.gateway.received()[1].body;
    assert_eq!(body["aps"]["alert"]["title"], ALERT_TITLE);
    assert_eq!(body["aps"]["alert"]["body"], ALERT_BODY);
    assert_eq!(body["koan-e2e"], "c2VjcmV0");

    // A sender cannot add text of its own.
    let r = h
        .post(
            "/v1/push",
            json!({ "handle": handle, "kind": "notify", "title": "Your bank needs you" }),
        )
        .await;
    assert_eq!(r.status(), 400);
    assert_eq!(h.gateway.received().len(), 2);
}

#[tokio::test]
async fn a_blob_rides_only_on_a_notification_and_only_as_base64url() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    assert_eq!(h.push(&handle, "wake", Some("abc")).await.status(), 400);
    assert_eq!(
        h.push(&handle, "notify", Some("not base64!"))
            .await
            .status(),
        400
    );
    assert_eq!(
        h.push(&handle, "notify", Some(&"A".repeat(4000)))
            .await
            .status(),
        400
    );
    assert_eq!(h.gateway.received().len(), 1, "only registration's proof");
}

#[tokio::test]
async fn a_handle_the_relay_did_not_seal_is_gone() {
    let h = Harness::start(|_| {}).await;
    for handle in ["", "nonsense", &"A".repeat(120), TOKEN] {
        assert_eq!(h.push(handle, "wake", None).await.status(), 410, "{handle}");
    }
    let mut forged = h.register(TOKEN).await.into_bytes();
    let last = forged.len() - 2;
    forged[last] = if forged[last] == b'A' { b'B' } else { b'A' };
    let forged = String::from_utf8(forged).unwrap();
    assert_eq!(h.push(&forged, "wake", None).await.status(), 410);
    assert_eq!(h.gateway.received().len(), 1, "only registration's proof");
}

#[tokio::test]
async fn each_server_has_its_own_allowance_under_the_devices_ceiling() {
    let h = Harness::start(|cfg| {
        cfg.limits.handle_alerts_per_hour = 2;
        cfg.limits.device_alerts_per_hour = 3;
    })
    .await;
    let hostile = h.register_for(TOKEN, &"11".repeat(16)).await;
    let friendly = h.register_for(TOKEN, &"22".repeat(16)).await;

    for _ in 0..2 {
        assert_eq!(h.push(&hostile, "summon", None).await.status(), 204);
    }
    let r = h.push(&hostile, "summon", None).await;
    assert_eq!(r.status(), 429);
    let retry: u64 = r.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=1800).contains(&retry), "{retry}");

    // The server that spent its allowance has not spent the other's.
    assert_eq!(h.push(&friendly, "summon", None).await.status(), 204);
    // But the device's ceiling holds across both.
    assert_eq!(h.push(&friendly, "summon", None).await.status(), 429);

    // Wakes are counted apart.
    assert_eq!(h.push(&hostile, "wake", None).await.status(), 204);
    // And another device has its own count.
    let other = h.register(&"ff".repeat(32)).await;
    assert_eq!(h.push(&other, "summon", None).await.status(), 204);
}

#[tokio::test]
async fn a_renewed_handle_shares_its_servers_allowance() {
    let h = Harness::start(|cfg| cfg.limits.handle_alerts_per_hour = 1).await;
    let first = h.register(TOKEN).await;
    let renewed = h.register(TOKEN).await;
    assert_ne!(first, renewed);
    assert_eq!(h.push(&first, "notify", None).await.status(), 204);
    assert_eq!(h.push(&renewed, "notify", None).await.status(), 429);
}

#[tokio::test]
async fn a_sender_that_hangs_up_still_pays() {
    let h = Harness::start(|cfg| cfg.limits.handle_alerts_per_hour = 1).await;
    let handle = h.register(TOKEN).await;
    *h.gateway.delay.lock() = std::time::Duration::from_millis(500);
    let impatient = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(100))
        .build()
        .unwrap();
    let hung_up = impatient
        .post(format!("{}/v1/push", h.url))
        .json(&json!({ "handle": handle, "kind": "notify" }))
        .send()
        .await;
    assert!(hung_up.is_err());
    // The push went on without its sender, and was paid for.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(h.gateway.received().len(), 2);
    *h.gateway.delay.lock() = std::time::Duration::ZERO;
    assert_eq!(h.push(&handle, "notify", None).await.status(), 429);
}

#[tokio::test]
async fn only_json_is_taken() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    for path in ["/v1/register", "/v1/revoke", "/v1/push"] {
        // What a web page can send a visitor's browser without a preflight.
        let r = h
            .http
            .post(format!("{}{path}", h.url))
            .header("content-type", "text/plain")
            .body(
                json!({ "token": TOKEN, "slot": SLOT, "handle": handle, "kind": "wake", "revoker": handle })
                    .to_string(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 415, "{path}");
    }
    assert_eq!(h.gateway.received().len(), 1, "only registration's proof");
}

#[tokio::test]
async fn a_registration_needs_a_slot() {
    let h = Harness::start(|_| {}).await;
    for body in [
        json!({ "token": TOKEN }),
        json!({ "token": TOKEN, "slot": "abc" }),
        json!({ "token": TOKEN, "slot": "zz".repeat(16) }),
    ] {
        assert_eq!(h.post("/v1/register", body).await.status(), 400);
    }
}

#[tokio::test]
async fn a_push_says_which_server_it_came_through() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    assert_eq!(
        h.push(&handle, "notify", Some("YmxvYg")).await.status(),
        204
    );
    let got = &h.gateway.received()[1];
    assert_eq!(got.body["koan-from"], SLOT);
    assert_eq!(
        got.headers["apns-collapse-id"].to_str().unwrap(),
        format!("koan-play-{SLOT}")
    );
}
#[tokio::test]
async fn a_push_apple_could_not_take_spends_no_allowance() {
    let h = Harness::start(|cfg| cfg.limits.handle_alerts_per_hour = 1).await;
    let handle = h.register(TOKEN).await;
    h.gateway.answer(500, "InternalServerError");
    for _ in 0..3 {
        assert_eq!(h.push(&handle, "notify", None).await.status(), 503);
    }
    h.gateway.accept();
    assert_eq!(h.push(&handle, "notify", None).await.status(), 204);
    assert_eq!(h.push(&handle, "notify", None).await.status(), 429);
}

#[tokio::test]
async fn the_request_limit_is_one_limit_across_routes() {
    let h = Harness::start(|cfg| cfg.limits.max_requests = 1).await;
    let handle = h.register(TOKEN).await;
    *h.gateway.delay.lock() = std::time::Duration::from_secs(5);
    let before = h.gateway.arrived.load(std::sync::atomic::Ordering::SeqCst);
    let slow = {
        let http = h.http.clone();
        let url = format!("{}/v1/push", h.url);
        tokio::spawn(async move {
            http.post(url)
                .json(&json!({ "handle": handle, "kind": "wake" }))
                .send()
                .await
                .unwrap()
                .status()
        })
    };
    // The push holds the one slot once the gateway has it.
    while h.gateway.arrived.load(std::sync::atomic::Ordering::SeqCst) == before {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let r = h
        .post("/v1/register", json!({ "token": TOKEN, "slot": SLOT }))
        .await;
    assert_eq!(r.status(), 503);
    assert!(r.headers().contains_key("retry-after"));
    assert_eq!(slow.await.unwrap(), 204);
}

#[tokio::test]
async fn a_revocation_retires_every_earlier_handle() {
    let h = Harness::start(|_| {}).await;
    let (old, revoker) = h.register_with_revoker(TOKEN, SLOT).await;
    let other_server = h.register_for(TOKEN, &"77".repeat(16)).await;
    let reached = h.gateway.received().len();
    let r = h.post("/v1/revoke", json!({ "revoker": revoker })).await;
    assert_eq!(r.status(), 204);
    assert_eq!(
        h.gateway.received().len(),
        reached,
        "revoking asks nothing of Apple"
    );
    assert_eq!(h.push(&old, "wake", None).await.status(), 410);
    assert_eq!(h.push(&other_server, "wake", None).await.status(), 410);
    // A revoker revokes once: it is retired with the rest.
    let r = h.post("/v1/revoke", json!({ "revoker": revoker })).await;
    assert_eq!(r.status(), 410);

    let new = h.register(TOKEN).await;
    assert_eq!(h.push(&new, "wake", None).await.status(), 204);
}

#[tokio::test]
async fn only_a_token_apple_accepts_gets_a_handle() {
    let h = Harness::start(|_| {}).await;
    h.gateway.answer(400, "BadDeviceToken");
    let r = h
        .post("/v1/register", json!({ "token": TOKEN, "slot": SLOT }))
        .await;
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn apple_refusing_the_relay_asks_senders_to_wait_minutes() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    h.gateway.answer(400, "TopicDisallowed");
    let r = h.push(&handle, "wake", None).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.headers()["retry-after"], "300");
}

#[tokio::test]
async fn proving_a_token_counts_against_its_wakes() {
    let h = Harness::start(|cfg| cfg.limits.device_wakes_per_hour = 2).await;
    h.register(TOKEN).await;
    h.register(TOKEN).await;
    let r = h
        .post("/v1/register", json!({ "token": TOKEN, "slot": SLOT }))
        .await;
    assert_eq!(r.status(), 429);
    assert_eq!(h.gateway.received().len(), 2);
}

#[tokio::test]
async fn a_signing_key_apple_rejects_asks_senders_to_wait_minutes() {
    for (status, reason) in [
        (403, "InvalidProviderToken"),
        (429, "TooManyProviderTokenUpdates"),
    ] {
        let h = Harness::start(|_| {}).await;
        let handle = h.register(TOKEN).await;
        h.gateway.answer(status, reason);
        let r = h.push(&handle, "wake", None).await;
        assert_eq!(r.status(), 503, "{reason}");
        assert_eq!(r.headers()["retry-after"], "300", "{reason}");
    }
}
#[tokio::test]
async fn an_absurd_lifetime_is_refused_not_wrapped() {
    for days in [0, 366, u64::MAX / 1000] {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config {
            store: dir.path().join("relay.db"),
            token_key: STANDARD.encode([3u8; 32]),
            ..Config::default()
        };
        cfg.handles.keys = BTreeMap::from([("1".to_string(), STANDARD.encode([5u8; 32]))]);
        cfg.handles.lifetime_days = days;
        cfg.apns.team_id = "TEAM123456".into();
        cfg.apns.production = Some(ApnsKey {
            key_id: "KEY1234567".into(),
            key: Some(pem()),
            key_path: None,
        });
        assert!(Relay::new(&cfg).is_err(), "{days}");
    }
}

#[tokio::test]
async fn only_the_apps_revoker_revokes() {
    let h = Harness::start(|_| {}).await;
    let (handle, revoker) = h.register_with_revoker(TOKEN, SLOT).await;
    // What a server holds is not enough, and neither is the raw token.
    let r = h.post("/v1/revoke", json!({ "revoker": handle })).await;
    assert_eq!(r.status(), 400);
    let r = h.post("/v1/revoke", json!({ "token": TOKEN })).await;
    assert_eq!(r.status(), 400);
    // And a revoker cannot be pushed through.
    assert_eq!(h.push(&revoker, "summon", None).await.status(), 400);
    assert_eq!(h.push(&handle, "wake", None).await.status(), 204);

    // Registration's proof is a bare wake, which the app, open and
    // registering, ignores.
    let probe = &h.gateway.received()[0];
    assert_eq!(probe.token, TOKEN);
    assert_eq!(probe.body, json!({ "aps": { "content-available": 1 } }));
}

#[tokio::test]
async fn apples_answers_reach_the_sender_as_the_relays() {
    let h = Harness::start(|_| {}).await;
    let cases = [
        (410, "Unregistered", 410),
        (429, "TooManyRequests", 429),
        (500, "InternalServerError", 503),
    ];
    for (i, (status, reason, expected)) in cases.into_iter().enumerate() {
        // A device of its own each: a gone device's handles are retired.
        let token = format!("{:064x}", i + 1);
        h.gateway.accept();
        let handle = h.register(&token).await;
        h.gateway.answer(status, reason);
        let r = h.push(&handle, "wake", None).await;
        assert_eq!(r.status(), expected, "{reason}");
        if expected == 503 {
            assert!(r.headers().contains_key("retry-after"));
        }
    }
}

#[tokio::test]
async fn a_proven_token_apple_now_refuses_is_retired() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    let other = h.register(&"ff".repeat(32)).await;
    h.gateway.answer(400, "BadDeviceToken");
    assert_eq!(h.push(&handle, "wake", None).await.status(), 410);
    h.gateway.accept();
    // That device is retired; the gateway carries on for everyone else.
    let reached = h.gateway.received().len();
    assert_eq!(h.push(&handle, "wake", None).await.status(), 410);
    assert_eq!(h.gateway.received().len(), reached);
    assert_eq!(h.push(&other, "wake", None).await.status(), 204);
}

#[tokio::test]
async fn a_burst_to_one_stale_device_does_not_stop_the_gateway() {
    let h = Harness::start(|_| {}).await;
    let stale = h.register(TOKEN).await;
    let other = h.register(&"ff".repeat(32)).await;
    h.gateway.answer(400, "BadDeviceToken");
    *h.gateway.delay.lock() = std::time::Duration::from_millis(200);
    let burst: Vec<_> = (0..8)
        .map(|_| {
            let http = h.http.clone();
            let url = format!("{}/v1/push", h.url);
            let stale = stale.clone();
            tokio::spawn(async move {
                http.post(url)
                    .json(&json!({ "handle": stale, "kind": "wake" }))
                    .send()
                    .await
                    .unwrap()
                    .status()
            })
        })
        .collect();
    for push in burst {
        assert_eq!(push.await.unwrap(), 410);
    }
    *h.gateway.delay.lock() = std::time::Duration::ZERO;
    h.gateway.accept();
    assert_eq!(h.push(&other, "wake", None).await.status(), 204);
}

#[tokio::test]
async fn proven_tokens_refused_across_devices_stop_the_gateway() {
    let h = Harness::start(|_| {}).await;
    let mut handles = Vec::new();
    for i in 0..6 {
        handles.push(h.register(&format!("{:064x}", i + 1)).await);
    }
    // What Apple says to every token when the relay's topic or gateway is
    // wrong.
    h.gateway.answer(400, "DeviceTokenNotForTopic");
    for handle in &handles[..5] {
        assert_eq!(h.push(handle, "wake", None).await.status(), 410);
    }
    h.gateway.accept();
    let reached = h.gateway.received().len();
    let r = h.push(&handles[5], "wake", None).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.headers()["retry-after"], "300");
    assert_eq!(h.gateway.received().len(), reached, "held, not sent");
}

#[tokio::test]
async fn an_answer_the_relay_does_not_know_does_not_stop_the_gateway() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    h.gateway.answer(400, "SomethingApplesAddedLater");
    let r = h.push(&handle, "wake", None).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.headers()["retry-after"], "300");
    h.gateway.accept();
    assert_eq!(h.push(&handle, "wake", None).await.status(), 204);
}

#[tokio::test]
async fn an_expired_token_signed_moments_ago_is_the_clock_not_apple() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    h.gateway.answer(403, "ExpiredProviderToken");
    let r = h.push(&handle, "wake", None).await;
    assert_eq!(r.status(), 503);
    assert_eq!(
        r.headers()["retry-after"],
        "300",
        "held, not re-signed in a loop"
    );
    let reached = h.gateway.received().len();
    assert_eq!(h.push(&handle, "wake", None).await.status(), 503);
    assert_eq!(h.gateway.received().len(), reached);
}

#[tokio::test]
async fn behind_a_proxy_a_request_without_a_caller_is_refused() {
    let h = Harness::start(|cfg| cfg.client_ip_header = Some("cf-connecting-ip".into())).await;
    let r = h
        .post("/v1/register", json!({ "token": TOKEN, "slot": SLOT }))
        .await;
    assert_eq!(r.status(), 400);
    let r = h
        .http
        .post(format!("{}/v1/register", h.url))
        .header("cf-connecting-ip", "1.2.3.4:5678")
        .json(&json!({ "token": TOKEN, "slot": SLOT }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert!(h.gateway.received().is_empty());
}

#[tokio::test]
async fn a_device_apple_calls_gone_is_never_sent_to_again() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    h.gateway.answer(410, "Unregistered");
    assert_eq!(h.push(&handle, "wake", None).await.status(), 410);
    let reached = h.gateway.received().len();
    h.gateway.accept();
    assert_eq!(h.push(&handle, "summon", None).await.status(), 410);
    assert_eq!(h.gateway.received().len(), reached);
}

#[tokio::test]
async fn a_refused_key_on_one_gateway_leaves_the_other_sending() {
    let h = Harness::start(|cfg| {
        cfg.apns.sandbox = Some(ApnsKey {
            key_id: "SANDBOX123".into(),
            key: Some(pem()),
            key_path: None,
        });
        cfg.apns.sandbox_url = cfg.apns.production_url.clone();
    })
    .await;
    let production = h.register(TOKEN).await;
    let r = h
        .post(
            "/v1/register",
            json!({ "token": "ff".repeat(32), "sandbox": true, "slot": SLOT }),
        )
        .await;
    let sandbox = r.json::<Value>().await.unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_string();
    h.gateway.answer(403, "InvalidProviderToken");
    assert_eq!(h.push(&sandbox, "wake", None).await.status(), 503);
    h.gateway.accept();
    assert_eq!(h.push(&production, "wake", None).await.status(), 204);
    let reached = h.gateway.received().len();
    assert_eq!(h.push(&sandbox, "wake", None).await.status(), 503);
    assert_eq!(
        h.gateway.received().len(),
        reached,
        "the sandbox stays held"
    );
}
#[tokio::test]
async fn a_relay_apple_refuses_stops_sending() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    h.gateway.answer(403, "InvalidProviderToken");
    assert_eq!(h.push(&handle, "wake", None).await.status(), 503);
    let reached = h.gateway.received().len();
    for _ in 0..5 {
        let r = h.push(&handle, "wake", None).await;
        assert_eq!(r.status(), 503);
        assert_eq!(r.headers()["retry-after"], "300");
    }
    assert_eq!(
        h.gateway.received().len(),
        reached,
        "refused here, not sent"
    );
    // Registration stops too: a proof would carry the same refused key.
    let r = h
        .post(
            "/v1/register",
            json!({ "token": "ff".repeat(32), "slot": SLOT }),
        )
        .await;
    assert_eq!(r.status(), 503);
    assert_eq!(h.gateway.received().len(), reached);
}

#[tokio::test]
async fn an_address_sending_made_up_tokens_stops_proving_them() {
    let h = Harness::start(|cfg| {
        cfg.limits.ip_bad_tokens_per_hour = 2;
        cfg.client_ip_header = Some("x-forwarded-for".into());
    })
    .await;
    let register = |from: &'static str, token: String| {
        h.http
            .post(format!("{}/v1/register", h.url))
            .header("x-forwarded-for", from)
            .json(&json!({ "token": token, "slot": SLOT }))
            .send()
    };
    h.gateway.answer(400, "BadDeviceToken");
    for i in 0..2 {
        let r = register("198.51.100.1", format!("{:064x}", i + 1))
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
    }
    let reached = h.gateway.received().len();
    let r = register("198.51.100.1", "ee".repeat(32)).await.unwrap();
    assert_eq!(r.status(), 429);
    assert_eq!(h.gateway.received().len(), reached, "no proof sent");
    // Another address is not held by it.
    h.gateway.accept();
    assert_eq!(
        register("203.0.113.1", TOKEN.into())
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn made_up_tokens_from_everywhere_hold_registration_but_never_revocation() {
    let h = Harness::start(|cfg| {
        cfg.limits.bad_tokens_per_hour = 2;
        cfg.client_ip_header = Some("x-forwarded-for".into());
    })
    .await;
    let post = |path: &'static str, from: String, body: Value| {
        h.http
            .post(format!("{}{path}", h.url))
            .header("x-forwarded-for", from)
            .json(&body)
            .send()
    };
    let r = post(
        "/v1/register",
        "203.0.113.1".into(),
        json!({ "token": TOKEN, "slot": SLOT }),
    )
    .await
    .unwrap();
    let revoker = r.json::<Value>().await.unwrap()["revoker"]
        .as_str()
        .unwrap()
        .to_string();
    h.gateway.answer(400, "BadDeviceToken");
    for i in 0..2 {
        let token = format!("{:064x}", i + 1);
        let r = post(
            "/v1/register",
            format!("198.51.100.{i}"),
            json!({ "token": token, "slot": SLOT }),
        )
        .await
        .unwrap();
        assert_eq!(r.status(), 400);
    }
    h.gateway.accept();
    let reached = h.gateway.received().len();
    let r = post(
        "/v1/register",
        "203.0.113.1".into(),
        json!({ "token": TOKEN, "slot": SLOT }),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 503);
    assert_eq!(h.gateway.received().len(), reached, "no proof sent");
    let r = post(
        "/v1/revoke",
        "203.0.113.1".into(),
        json!({ "revoker": revoker }),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 204);
}

#[tokio::test]
async fn ipv6_registrations_are_counted_by_slash_48() {
    let h = Harness::start(|cfg| {
        cfg.limits.ip_registrations_per_minute = 1;
        cfg.client_ip_header = Some("x-forwarded-for".into());
    })
    .await;
    let register = |forwarded: &'static str| {
        h.http
            .post(format!("{}/v1/register", h.url))
            .header("x-forwarded-for", forwarded)
            .json(&json!({ "token": TOKEN, "slot": SLOT }))
            .send()
    };
    assert_eq!(register("2001:db8:0:1::1").await.unwrap().status(), 200);
    assert_eq!(register("2001:db8:0:2::1").await.unwrap().status(), 429);
    assert_eq!(register("2001:db8:1::1").await.unwrap().status(), 200);
}

#[tokio::test]
async fn spending_a_devices_wakes_does_not_stop_it_revoking() {
    let h = Harness::start(|cfg| cfg.limits.device_wakes_per_hour = 1).await;
    let (_, revoker) = h.register_with_revoker(TOKEN, SLOT).await;
    let r = h
        .post("/v1/register", json!({ "token": TOKEN, "slot": SLOT }))
        .await;
    assert_eq!(r.status(), 429);
    let r = h.post("/v1/revoke", json!({ "revoker": revoker })).await;
    assert_eq!(r.status(), 204);
}

#[tokio::test]
async fn relayed_notifications_do_not_break_through_a_focus() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    for kind in ["summon", "notify"] {
        assert_eq!(h.push(&handle, kind, None).await.status(), 204);
    }
    for got in &h.gateway.received()[1..] {
        assert_eq!(got.body["aps"]["interruption-level"], "active");
        assert!(
            got.headers["apns-collapse-id"]
                .to_str()
                .unwrap()
                .ends_with(SLOT)
        );
    }
}
#[tokio::test]
async fn a_gateway_without_a_key_is_refused_at_registration() {
    let h = Harness::start(|_| {}).await;
    let r = h
        .post(
            "/v1/register",
            json!({ "token": TOKEN, "sandbox": true, "slot": SLOT }),
        )
        .await;
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn malformed_tokens_and_oversized_bodies_are_refused() {
    let h = Harness::start(|_| {}).await;
    for token in ["", "abc", "zz", &"ab".repeat(101)] {
        let r = h
            .post("/v1/register", json!({ "token": token, "slot": SLOT }))
            .await;
        assert_eq!(r.status(), 400, "{token}");
    }
    let r = h
        .post(
            "/v1/push",
            json!({ "handle": "A".repeat(10_000), "kind": "wake" }),
        )
        .await;
    assert_eq!(r.status(), 413);
}

#[tokio::test]
async fn registrations_are_limited_per_address() {
    let h = Harness::start(|cfg| cfg.limits.ip_registrations_per_minute = 2).await;
    h.register(TOKEN).await;
    h.register(TOKEN).await;
    let r = h
        .post("/v1/register", json!({ "token": TOKEN, "slot": SLOT }))
        .await;
    assert_eq!(r.status(), 429);
}

#[tokio::test]
async fn behind_a_proxy_the_last_forwarded_address_is_the_caller() {
    let h = Harness::start(|cfg| {
        cfg.limits.ip_registrations_per_minute = 1;
        cfg.client_ip_header = Some("x-forwarded-for".into());
    })
    .await;
    let register = |forwarded: &'static str| {
        h.http
            .post(format!("{}/v1/register", h.url))
            .header("x-forwarded-for", forwarded)
            .json(&json!({ "token": TOKEN, "slot": SLOT }))
            .send()
    };
    assert_eq!(
        register("1.1.1.1, 203.0.113.1").await.unwrap().status(),
        200
    );
    // The caller cannot dodge the limit by prepending an address.
    assert_eq!(
        register("2.2.2.2, 203.0.113.1").await.unwrap().status(),
        429
    );
    assert_eq!(register("203.0.113.2").await.unwrap().status(), 200);
    // One IPv6 /64 is one caller.
    assert_eq!(register("2001:db8::1").await.unwrap().status(), 200);
    assert_eq!(register("2001:db8::2").await.unwrap().status(), 429);
}

#[tokio::test]
async fn behind_two_proxies_the_caller_is_two_back() {
    let h = Harness::start(|cfg| {
        cfg.limits.ip_registrations_per_minute = 1;
        cfg.client_ip_header = Some("x-forwarded-for".into());
        cfg.client_ip_hops = 2;
    })
    .await;
    let register = |forwarded: &'static str| {
        h.http
            .post(format!("{}/v1/register", h.url))
            .header("x-forwarded-for", forwarded)
            .json(&json!({ "token": TOKEN, "slot": SLOT }))
            .send()
    };
    // Callers behind one edge proxy are told apart by what it appended.
    assert_eq!(
        register("9.9.9.9, 198.51.100.1, 10.0.0.1")
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        register("198.51.100.2, 10.0.0.1").await.unwrap().status(),
        200
    );
    assert_eq!(
        register("8.8.8.8, 198.51.100.1, 10.0.0.1")
            .await
            .unwrap()
            .status(),
        429
    );
}

#[tokio::test]
async fn a_proxys_own_header_line_wins_over_the_callers() {
    let h = Harness::start(|cfg| {
        cfg.limits.ip_registrations_per_minute = 1;
        cfg.client_ip_header = Some("x-forwarded-for".into());
    })
    .await;
    let register = |forged: &'static str| {
        h.http
            .post(format!("{}/v1/register", h.url))
            .header("x-forwarded-for", forged)
            .header("x-forwarded-for", "203.0.113.9")
            .json(&json!({ "token": TOKEN, "slot": SLOT }))
            .send()
    };
    assert_eq!(register("1.1.1.1").await.unwrap().status(), 200);
    assert_eq!(register("2.2.2.2").await.unwrap().status(), 429);
}

#[tokio::test]
async fn metrics_count_outcomes_and_nothing_else() {
    let h = Harness::start(|_| {}).await;
    let handle = h.register(TOKEN).await;
    h.push(&handle, "wake", None).await;
    h.push("nonsense", "wake", None).await;
    let text = h
        .http
        .get(&h.metrics)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.contains(r#"koan_relay_requests_total{route="push",outcome="ok"} 1"#));
    assert!(text.contains(r#"koan_relay_requests_total{route="push",outcome="gone"} 1"#));
    assert!(text.contains(r#"koan_relay_pushes_sent_total{kind="wake"} 1"#));
    assert!(!text.contains(TOKEN) && !text.contains(&handle));
}

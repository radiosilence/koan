//! Pairing: signing in a device with no keyboard, such as a television.
//!
//! The device opens `/rest/koanPair`, unauthenticated, and is held there with
//! a pairing: an unguessable id for a link and a short code to read off the
//! screen. Someone signed in approves it through `/rest/koanPairApprove` or
//! the web UI's `/pair`, and an API key made on their account goes down the
//! waiting socket. Pairings live in memory only: one outlives neither its
//! socket nor ten minutes, so a restart loses nothing a device cannot ask for
//! again. The registry is `koan_auth::pair`'s; the socket and its messages
//! are kōan's. See `koan_core::remote::pair` for the device's side.

use std::sync::LazyLock;
use std::time::Duration;

use axum::extract::RawQuery;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use koan_core::remote::pair::PairMessage;

use crate::auth::routes::RateLimiter;

pub use koan_auth::pair::{OpenError, Opened, PairError, PairInfo, Pairings, Settled, TTL, Taken};

/// One set per process: the socket route, the Subsonic endpoints and the web
/// UI all reach the same pairings.
pub fn pairings() -> &'static Pairings {
    static PAIRINGS: LazyLock<Pairings> = LazyLock::new(|| Pairings::new(ttl_from_env()));
    &PAIRINGS
}

/// `KOAN_PAIR_TTL_SECS`, or `TTL`. For trying expiry without waiting ten
/// minutes; read once, when the first pairing is asked for.
fn ttl_from_env() -> Duration {
    std::env::var("KOAN_PAIR_TTL_SECS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|&secs| secs > 0)
        .map_or(TTL, Duration::from_secs)
}

/// Pairings opened per address in a minute. Each one is a held socket, and a
/// device needs one.
static OPENS: LazyLock<RateLimiter> = LazyLock::new(|| RateLimiter::new(60, 10));

/// `/rest/koanPair?name=…&device=…`: a device asking to be signed in. Unauthenticated,
/// since it has nothing to sign in with yet.
pub(crate) async fn route(
    RawQuery(raw): RawQuery,
    axum::Extension(stop): axum::Extension<tokio_util::sync::CancellationToken>,
    ws: WebSocketUpgrade,
    request: axum::extract::Request,
) -> Response {
    // Browsers send an Origin with every WebSocket and apps do not. Without
    // this, a page someone on the network visits could open a pairing from
    // their address, have them approve it as "on your network", and read the
    // key it was sent.
    if request.headers().contains_key(axum::http::header::ORIGIN) {
        return (StatusCode::FORBIDDEN, "pairing is not open to web pages").into_response();
    }
    let from = crate::auth::routes::client_ip(&request);
    if !OPENS.allow(from) {
        return (StatusCode::TOO_MANY_REQUESTS, "too many pairings").into_response();
    }
    let query: Vec<(String, String)> = form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
        .into_owned()
        .collect();
    let param = |key: &str| {
        query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    match pairings().open(param("name").unwrap_or_default(), param("device"), from) {
        Ok(opened) => ws.on_upgrade(move |socket| session(socket, opened, stop)),
        Err(OpenError::Full) => {
            (StatusCode::SERVICE_UNAVAILABLE, "too many pairings waiting").into_response()
        }
        Err(OpenError::Busy) => (
            StatusCode::TOO_MANY_REQUESTS,
            "too many pairings waiting from this address",
        )
            .into_response(),
        Err(OpenError::Entropy) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Hold the device until its pairing is settled, then tell it the outcome,
/// and tell an approval whether the socket took it.
async fn session(
    mut socket: WebSocket,
    mut opened: Opened<'static>,
    stop: tokio_util::sync::CancellationToken,
) {
    let waited = tokio::select! {
        waited = wait(&mut socket, &mut opened) => waited,
        // The pairing goes with the server; the device asks again.
        () = stop.cancelled() => None,
    };
    // Nothing reads the outcome after this, so `settle` now finds the device
    // gone. One settled just before is read here; dropped unsent, as when
    // the device went first, its approval is revoked by `settle`.
    opened.outcome.close();
    let late = opened.outcome.try_recv().ok();
    let Some(waited) = waited else { return };
    // A lapse can race an approval, which then wins.
    let settled = late.unwrap_or(waited);
    log::info!(
        "pair: {} {}",
        opened.code,
        match settled.outcome {
            koan_auth::pair::Outcome::Approved { .. } => "approved",
            koan_auth::pair::Outcome::Declined => "declined",
            koan_auth::pair::Outcome::Expired => "expired",
        }
    );
    let sent = send(&mut socket, &settled.outcome.clone().into())
        .await
        .is_ok();
    settled.delivered(sent);
    let _ = socket.send(Message::Close(None)).await;
}

/// Wait for the pairing's outcome. `None` when the device went first: it
/// closed the socket, or stopped answering pings.
async fn wait(socket: &mut WebSocket, opened: &mut Opened<'static>) -> Option<Settled> {
    let pending = PairMessage::Pending {
        id: opened.id.clone(),
        code: opened.code.clone(),
        expires_in: opened.pairings().ttl().as_secs(),
    };
    send(socket, &pending).await.ok()?;
    log::info!("pair: {} waiting", opened.code);
    let lapse = tokio::time::sleep(opened.pairings().ttl());
    tokio::pin!(lapse);
    let keepalive = opened.pairings().keepalive();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + keepalive, keepalive);
    let silence = opened.pairings().silence();
    let quiet = tokio::time::sleep(silence);
    tokio::pin!(quiet);
    loop {
        tokio::select! {
            // A dropped sender is a sweep that found it lapsed.
            outcome = &mut opened.outcome => {
                return Some(outcome.unwrap_or_else(|_| koan_auth::pair::Outcome::Expired.into()));
            }
            () = &mut lapse => return Some(koan_auth::pair::Outcome::Expired.into()),
            () = &mut quiet => {
                log::info!("pair: {} stopped answering", opened.code);
                return None;
            }
            _ = ping.tick() => socket.send(Message::Ping(Default::default())).await.ok()?,
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                Some(Ok(_)) => quiet.as_mut().reset(tokio::time::Instant::now() + silence),
            },
        }
    }
}

async fn send(socket: &mut WebSocket, message: &PairMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).unwrap_or_default();
    socket.send(Message::Text(text.into())).await
}

/// Stands in for a device's session, on a thread of its own: takes the
/// outcome and says whether the socket took it, as `sent`.
#[cfg(test)]
pub(crate) fn device(
    mut opened: Opened<'static>,
    sent: bool,
) -> std::thread::JoinHandle<PairMessage> {
    std::thread::spawn(move || {
        // `Opened` is `Drop`, so its receiver is swapped out, not moved.
        let outcome = std::mem::replace(&mut opened.outcome, tokio::sync::oneshot::channel().1);
        let settled = outcome.blocking_recv().unwrap();
        let message = PairMessage::from(settled.outcome.clone());
        settled.delivered(sent);
        message
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use tokio::sync::oneshot;

    const ME: i64 = 1;
    const LAN: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 20));

    fn database() -> (tempfile::TempDir, koan_core::db::connection::Database, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db = koan_core::db::connection::Database::open(&dir.path().join("koan.db")).unwrap();
        let id = koan_core::db::queries::auth::create_user(
            &db.conn,
            "alice",
            "hunter2",
            koan_core::auth::Role::Readonly,
        )
        .unwrap();
        (dir, db, id)
    }

    /// Serves pairings from `p` the way `route` does.
    async fn serve(p: &'static Pairings) -> std::net::SocketAddr {
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: WebSocketUpgrade| async move {
                let opened = p.open("tv", None, LAN).unwrap();
                ws.on_upgrade(move |socket| session(socket, opened, Default::default()))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        addr
    }

    fn quick(keepalive: Duration) -> &'static Pairings {
        Box::leak(Box::new(Pairings::new(TTL).with_keepalive(keepalive)))
    }

    fn pending_id(
        socket: &mut tungstenite::WebSocket<impl std::io::Read + std::io::Write>,
    ) -> String {
        let first = socket.read().unwrap();
        match serde_json::from_str(first.to_text().unwrap()).unwrap() {
            PairMessage::Pending { id, .. } => id,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_that_stops_answering_is_dropped() {
        // Seconds, not milliseconds: a busy runner must not make a device
        // that answers look silent.
        let p = quick(Duration::from_secs(1));
        let addr = serve(p).await;
        // Connected, then never read again, so pings go unanswered.
        let (socket, id) = tokio::task::spawn_blocking(move || {
            let (mut socket, _) = tungstenite::connect(format!("ws://{addr}/")).unwrap();
            let id = pending_id(&mut socket);
            (socket, id)
        })
        .await
        .unwrap();
        assert!(p.info(&id, ME).is_ok());
        tokio::time::sleep(p.silence() * 2).await;
        assert_eq!(p.info(&id, ME).err(), Some(PairError::NotFound));
        drop(socket);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_that_answers_waits_and_gets_its_key() {
        let (_dir, db, alice) = database();
        let p = quick(Duration::from_secs(1));
        let addr = serve(p).await;
        let (id_tx, id_rx) = oneshot::channel();
        let device = tokio::task::spawn_blocking(move || {
            let (mut socket, _) = tungstenite::connect(format!("ws://{addr}/")).unwrap();
            id_tx.send(pending_id(&mut socket)).unwrap();
            // Reading answers the pings.
            loop {
                if let tungstenite::Message::Text(text) = socket.read().unwrap() {
                    return serde_json::from_str::<PairMessage>(&text).unwrap();
                }
            }
        });
        let id = id_rx.await.unwrap();
        tokio::time::sleep(p.silence() * 2).await;
        // `settle` blocks until the session has written the key.
        let db = tokio::task::spawn_blocking(move || {
            p.settle(&db.conn, &id, alice, "alice", false).unwrap();
            db
        })
        .await
        .unwrap();
        let PairMessage::Approved { api_key, .. } = device.await.unwrap() else {
            panic!("not approved");
        };
        assert!(
            koan_core::db::queries::api_keys::authenticate_api_key(&db.conn, &api_key)
                .unwrap()
                .is_some()
        );
    }
}

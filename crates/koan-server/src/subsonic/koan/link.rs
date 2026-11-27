//! The link: a koan client's standing connection to this server, and the
//! commands relayed down it.

use super::*;

/// A koan client linking itself to this server: authenticated like any other
/// call, then held open for as long as the client stays, so the server can
/// send it `LinkCommand`s.
pub(in crate::subsonic) async fn koan_link(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
    ws: axum::extract::WebSocketUpgrade,
    request: axum::extract::Request,
) -> Response {
    // Where the device is, as the rate limits see it: what lets a device wake
    // another account's on the same network.
    let addr = crate::auth::routes::client_ip(&request);
    let params = RawParams::parse(raw.as_deref());
    let json = params.auth().wants_json();
    let mark = koan_core::auth::account_mark();
    let caller = {
        let auth = params.auth();
        let state = state.clone();
        match tokio::task::spawn_blocking(move || validate_auth(&auth, &state)).await {
            Ok(Ok(caller)) => caller,
            Ok(Err(e)) => return SubsonicResponse::error(json, &e),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };
    let name = params.get("client").unwrap_or("koan").to_owned();
    let platform = params.get("platform").unwrap_or("").to_owned();
    // Without a device id every connection is its own device, and nothing of
    // it is kept once it closes.
    let remembered = params.get("device").is_some();
    let device = params
        .get("device")
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let wants_devices = params.get("devices") == Some("1");
    // It answers commands sent with an id: see `koan_core::remote::acks`.
    let acks = params.get("acks") == Some("1");
    // The public key the device proves itself with on the local network,
    // kept on the API key that signed it in, so revoking the key drops it.
    // A link signed with a password or the shared secret has no key row to
    // keep it on, and stays unproven there; so does a link without a device
    // id, which has no stable id to keep the key under.
    let device_key = params
        .get("deviceKey")
        .filter(|_| remembered)
        .map(str::to_owned);
    if let Some(public) = &device_key {
        if !koan_core::remote::link::valid_device_key(public) {
            log::warn!("link: {device} sent a device key that is not one; ignored");
        } else if let Some(raw) = params.auth().api_key {
            let (st, dev, public) = (state.clone(), device.clone(), public.clone());
            let kept = tokio::task::spawn_blocking(move || {
                let db = st.open_db().ok()?;
                koan_core::db::queries::api_keys::set_device_key(&db.conn, &raw, &dev, &public).ok()
            })
            .await;
            if !matches!(kept, Ok(Some(true))) {
                log::warn!("link: could not keep the device key for {device}");
            }
        }
    }
    let pool = state.pool.clone();
    let stop = state.stop.clone();
    let links = state.links.clone();
    let lease = crate::auth::Lease {
        user_id: caller.user_id,
        mark,
        expires: None,
    };
    ws.on_upgrade(move |socket| {
        links.track_future(link_session(
            socket,
            caller.username,
            lease,
            stop,
            LinkPeer {
                name,
                platform,
                device,
                wants_devices,
                acks,
                wants_keys: device_key.is_some(),
                addr,
                remembered,
            },
            pool,
        ))
    })
}

/// Who is at the far end of a link.
pub(in crate::subsonic) struct LinkPeer {
    name: String,
    platform: String,
    device: String,
    wants_devices: bool,
    acks: bool,
    /// It sent a device key, so it understands `LinkCommand::DeviceKeys`.
    wants_keys: bool,
    /// The client's address, through a trusted proxy if there is one.
    addr: std::net::IpAddr,
    /// It named itself with a device id. One that did not is a new device
    /// each time, so nothing is recorded of it to outlive its link.
    remembered: bool,
}

/// Keep every keyed link's list current as accounts change. A revoked key,
/// a signed-out device or a deleted account takes its public key with it,
/// and the accounts it was shared with are linked on their own, so nothing
/// else would tell them. Account changes are rare, and each sends every
/// keyed account its list again.
pub(in crate::subsonic) fn spawn_key_publisher(
    pool: Arc<Pool>,
    stop: tokio_util::sync::CancellationToken,
) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(stop.run_until_cancelled_owned(async move {
        let mut mark = koan_core::auth::account_mark();
        loop {
            mark = koan_core::auth::any_account_changed_since(mark).await;
            let pool = pool.clone();
            let _ = tokio::task::spawn_blocking(move || {
                for username in crate::clients::registry().keyed_accounts() {
                    publish_device_keys(&pool, &username);
                }
            })
            .await;
        }
    }));
}

/// Send `username`'s links that asked for them the device keys they check
/// the network against: the account's own, and devices shared with it.
/// Nothing for an account this database does not have: the registry is the
/// process's, and a link may be another database's.
pub(in crate::subsonic) fn publish_device_keys(pool: &Pool, username: &str) {
    let Ok(db) = pool.get() else { return };
    if !matches!(
        queries::auth::get_user_by_username(&db.conn, username),
        Ok(Some(_))
    ) {
        return;
    }
    let Ok(keys) = queries::api_keys::device_keys(&db.conn, username) else {
        return;
    };
    let keys = keys
        .into_iter()
        .map(|k| koan_core::remote::link::LinkDeviceKey {
            id: k.device,
            key: k.key,
            owner: k.owner,
        })
        .collect();
    crate::clients::registry().publish_keys(username, keys);
}

/// Hand a link command to another of the caller's devices, in one request:
/// what a phone has when iOS has taken its link down and a button on its lock
/// screen is pressed. `to` is the device's id, `command` the command as the
/// link carries it.
pub(in crate::subsonic) async fn koan_command(
    State(state): State<Arc<AppState>>,
    RawQuery(raw): RawQuery,
) -> Response {
    offload_response(move || {
        let params = RawParams::parse(raw.as_deref());
        let auth = params.auth();
        respond(&state, &auth, |caller, b| {
            require_write(caller.role)?;
            let to = params
                .get("to")
                .ok_or_else(|| SubsonicError::missing_param("to"))?;
            let command = params
                .get("command")
                .and_then(|c| koan_core::remote::link::parse_command(c).ok())
                .ok_or_else(|| SubsonicError::bad_param("command"))?;
            // The client's id when it sent one, so a device the command
            // reaches by another way too acts on it once.
            let ack = params
                .get("ack")
                .and_then(|a| a.parse::<u64>().ok())
                .unwrap_or_else(koan_core::remote::acks::next_id);
            let (tx, answer) = crossbeam_channel::bounded(1);
            crate::clients::registry()
                .relay_acked(
                    &caller.username,
                    None,
                    to,
                    command,
                    Some(crate::clients::Acking {
                        id: ack,
                        reply: Box::new(move |outcome| {
                            let _ = tx.send(outcome);
                        }),
                    }),
                )
                .map_err(|e| SubsonicError::not_found(&e))?;
            // A device that gives no answers, or none in time, gets no
            // `koanCommand`: sent, as a server before answers said.
            Ok(match answer.recv_timeout(COMMAND_ANSWER).ok().flatten() {
                Some(outcome) => b.child(ack_node(&outcome)),
                None => b,
            })
        })
    })
    .await
}

/// How long `koanCommand` waits for the device's answer: its first look, and
/// a push to wake it if its link has gone quiet.
pub(in crate::subsonic) const COMMAND_ANSWER: Duration = Duration::from_secs(4);

/// An answer as `koanCommand` gives it, the same shape as the link's.
pub(in crate::subsonic) fn ack_node(outcome: &koan_core::remote::acks::AckOutcome) -> XmlNode {
    use koan_core::remote::acks::AckOutcome;
    let node = XmlNode::new(Tag::KoanCommand);
    match outcome {
        AckOutcome::Done => node.attr("result", "done"),
        AckOutcome::Queued => node.attr("result", "queued"),
        AckOutcome::Refused { reason } => node.attr("result", "refused").attr("reason", reason),
        AckOutcome::Failed { error } => node.attr("result", "failed").attr("error", error),
    }
}

/// Hand `to`'s answer to the command `device` sent under `ack` back to
/// `device`, as `Acked`: the first answer only, however many ways it comes.
pub(in crate::subsonic) fn answer_once(
    username: &str,
    device: &str,
    to: &str,
    ack: u64,
) -> std::sync::Arc<dyn Fn(koan_core::remote::acks::AckOutcome) + Send + Sync> {
    let (username, device, to) = (username.to_owned(), device.to_owned(), to.to_owned());
    let answered = std::sync::atomic::AtomicBool::new(false);
    std::sync::Arc::new(move |outcome| {
        if answered.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        // News, like levels: to the asker's link if it is still there, and
        // never a push to wake it for.
        crate::clients::registry().send_live(
            &username,
            &device,
            koan_core::remote::link::LinkCommand::Acked {
                from: to.clone(),
                ack,
                outcome,
            },
        );
    })
}

pub(in crate::subsonic) const LINK_CHECK: Duration = Duration::from_secs(15);
/// How long one frame may take to go out before the device is taken for gone.
pub(in crate::subsonic) const LINK_SEND: Duration = Duration::from_secs(10);
/// Over twice the client's idle ping interval.
pub(in crate::subsonic) const LINK_SILENCE: Duration = Duration::from_secs(100);

/// A link until the device goes, a newer link from it replaces this one, or
/// the account changes (see `Lease`). The device reconnects after that last,
/// as it does after any close, and is authenticated afresh.
pub(in crate::subsonic) async fn link_session(
    mut socket: axum::extract::ws::WebSocket,
    username: String,
    lease: crate::auth::Lease,
    stop: tokio_util::sync::CancellationToken,
    peer: LinkPeer,
    pool: Arc<Pool>,
) {
    use axum::extract::ws::{CloseFrame, Message, close_code};
    let LinkPeer {
        name,
        platform,
        device,
        wants_devices,
        acks,
        wants_keys,
        addr,
        remembered,
    } = peer;
    let (tx, mut rx) = crate::clients::LinkTx::channel();
    let behind = tx.behind();
    let registry = crate::clients::registry();
    // Registry calls read and write the outbox tables, so they run off the
    // async workers. Each is awaited in turn: a device's reports must land in
    // the order it sent them.
    let (id, waiting) = {
        let (username, name, platform, device) = (
            username.clone(),
            name.clone(),
            platform.clone(),
            device.clone(),
        );
        let pool = pool.clone();
        let registered = tokio::task::spawn_blocking(move || {
            let register = if remembered {
                crate::clients::Registry::register
            } else {
                crate::clients::Registry::register_passing
            };
            let (id, waiting) = register(
                registry,
                &username,
                &name,
                &platform,
                &device,
                tx,
                wants_devices,
                acks,
            );
            // After `register`, which records the device the address is kept on.
            if remembered {
                registry.seen_at(&device, &username, addr);
            }
            if wants_keys {
                registry.wants_keys(&id);
            }
            // Its key may be new: the account hears it, and so does every
            // account this device is shared with.
            publish_device_keys(&pool, &username);
            for grantee in registry.grantees_of(&username, &device) {
                publish_device_keys(&pool, &grantee);
            }
            (id, waiting)
        })
        .await;
        match registered {
            Ok(registered) => registered,
            Err(_) => return,
        }
    };
    log::info!("link: {name} ({platform}) linked for {username}");
    // A client pings when it has heard nothing for a while. A phone the OS
    // has suspended never closes its socket, so one that goes quiet is gone.
    let mut last_heard = tokio::time::Instant::now();
    let mut check = tokio::time::interval(LINK_CHECK);
    let ended = lease.ended();
    tokio::pin!(ended);
    // What waited for the device goes out first, before anything sent since
    // it linked. A link that drops partway loses the rest, as any would.
    'session: {
        for envelope in waiting {
            let Ok(text) = serde_json::to_string(&envelope) else {
                continue;
            };
            let sent = tokio::time::timeout(LINK_SEND, socket.send(Message::Text(text.into())));
            if !matches!(sent.await, Ok(Ok(()))) {
                break 'session;
            }
        }
        loop {
            tokio::select! {
                _ = &mut ended => {
                    let close = socket.send(Message::Close(Some(CloseFrame {
                        code: close_code::NORMAL,
                        reason: "sign in again".into(),
                    })));
                    let _ = tokio::time::timeout(LINK_SEND, close).await;
                    break;
                }
                // Going away, so the device links again once the server is back.
                _ = stop.cancelled() => {
                    let close = socket.send(Message::Close(Some(CloseFrame {
                        code: close_code::AWAY,
                        reason: "server stopping".into(),
                    })));
                    let _ = tokio::time::timeout(LINK_SEND, close).await;
                    break;
                }
                _ = behind.notified() => {
                    log::info!("link: {name} ({platform}) fell behind; unlinked so it syncs afresh");
                    break;
                }
                _ = check.tick() => {
                    if last_heard.elapsed() > LINK_SILENCE {
                        break;
                    }
                }
                cmd = rx.recv() => {
                    // None: a newer connection from the same device replaced this one.
                    let Some(cmd) = cmd else { break };
                    let Ok(text) = serde_json::to_string(&cmd) else { continue };
                    // A device that stops reading fills its socket's buffer, and
                    // the send waits; the silence check cannot run meanwhile.
                    let sent = tokio::time::timeout(LINK_SEND, socket.send(Message::Text(text.into())));
                    if !matches!(sent.await, Ok(Ok(()))) {
                        break;
                    }
                }
                msg = socket.recv() => match msg {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(msg)) => {
                        last_heard = tokio::time::Instant::now();
                        use koan_core::remote::link::LinkReport;
                        if let Message::Text(text) = msg {
                            match serde_json::from_str(&text) {
                                Ok(LinkReport::State(state)) => {
                                    let id = id.clone();
                                    let _ = tokio::task::spawn_blocking(move || {
                                        registry.report(&id, state);
                                    })
                                    .await;
                                }
                                Ok(LinkReport::Push { .. }) if !remembered => {}
                                Ok(LinkReport::Push { handle }) => {
                                    let (username, device) = (username.clone(), device.clone());
                                    let _ = tokio::task::spawn_blocking(move || {
                                        registry.set_push(&username, &device, &handle);
                                    })
                                    .await;
                                }
                                // A notification tapped: what it was for goes down
                                // this link, in order with what follows, and with
                                // the ack id it was asked with.
                                Ok(LinkReport::Claim { id }) => {
                                    match crate::clients::claims::take(&device, &username, &id) {
                                        Some(envelope) => registry.send_kept(&username, &device, envelope),
                                        None => log::info!("link: {device} claimed nothing, or too late"),
                                    }
                                }
                                Ok(LinkReport::Command {
                                    to,
                                    command: koan_core::remote::link::LinkCommand::WatchLevels { on },
                                    ..
                                }) => {
                                    registry.watch_levels(&username, &device, &to, on);
                                }
                                // At the analyser's rate: relayed in place, with
                                // no database and no thread hop.
                                Ok(LinkReport::Levels { f }) => {
                                    registry.levels(&username, &device, f);
                                }
                                Ok(LinkReport::Command { to, command, ack }) => {
                                    // Relaying may push to a phone, which blocks.
                                    let (username, device) = (username.clone(), device.clone());
                                    tokio::task::spawn_blocking(move || {
                                        // The answer goes back to this device as `Acked`,
                                        // once, however the relay ends.
                                        let reply = ack.map(|ack| answer_once(&username, &device, &to, ack));
                                        let acking = reply.clone().map(|reply| crate::clients::Acking {
                                            id: ack.unwrap_or_default(),
                                            reply: Box::new(move |outcome| {
                                                if let Some(outcome) = outcome {
                                                    reply(outcome);
                                                }
                                            }),
                                        });
                                        if let Err(e) = registry.relay_acked(&username, Some(&device), &to, command, acking) {
                                            log::info!("link: relay to {to}: {e}");
                                            if let Some(reply) = reply {
                                                reply(koan_core::remote::acks::AckOutcome::Refused { reason: e });
                                            }
                                        }
                                    });
                                }
                                Ok(LinkReport::Received { ack }) => {
                                    registry.received(&username, &device, ack);
                                }
                                Ok(LinkReport::Ack { ack, outcome }) => {
                                    let (username, device) = (username.clone(), device.clone());
                                    tokio::task::spawn_blocking(move || {
                                        registry.answered(&username, &device, ack, outcome);
                                    });
                                }
                                Ok(LinkReport::Activity { handle, device: shown }) => {
                                    let activity = handle.zip(shown);
                                    let (username, device) = (username.clone(), device.clone());
                                    tokio::task::spawn_blocking(move || {
                                        registry.set_activity(&username, &device, activity);
                                    });
                                }
                                Ok(LinkReport::Wake { to, notify }) => {
                                    let (username, device) = (username.clone(), device.clone());
                                    tokio::task::spawn_blocking(move || {
                                        registry.wake(&username, &device, &to, notify);
                                    });
                                }
                                Ok(LinkReport::Share { grantee, allow }) => {
                                    // Only ever about the device sending it: an
                                    // owner shares a device from that device.
                                    let (username, device) = (username.clone(), device.clone());
                                    let pool = pool.clone();
                                    tokio::task::spawn_blocking(move || {
                                        let known = pool
                                            .get()
                                            .ok()
                                            .and_then(|db| {
                                                koan_core::db::queries::auth::get_user_by_username(&db.conn, &grantee)
                                                    .ok()
                                                    .flatten()
                                            })
                                            .is_some();
                                        if allow && !known {
                                            log::info!("share: {username} asked to share with {grantee}, who has no account here");
                                            registry.send_shares(
                                                &username,
                                                &device,
                                                Some(format!("There is no account called {grantee} on this server.")),
                                            );
                                            return;
                                        }
                                        if let Err(e) = registry.share(&username, &device, &grantee, allow) {
                                            log::info!("share: {e}");
                                            registry.send_shares(&username, &device, Some(e));
                                            return;
                                        }
                                        // The grantee now checks this device's key, or no longer does.
                                        publish_device_keys(&pool, &grantee);
                                    });
                                }
                                Ok(LinkReport::Forget { device: forgotten }) => {
                                    let username = username.clone();
                                    tokio::task::spawn_blocking(move || {
                                        if let Err(e) = registry.forget(&username, &forgotten) {
                                            log::info!("devices: {e}");
                                        }
                                    });
                                }
                                Ok(LinkReport::TvPairing { device: tv, pairing }) => {
                                    let username = username.clone();
                                    tokio::task::spawn_blocking(move || {
                                        if let Err(e) = registry.set_tv_pairing(&username, &tv, pairing.as_ref()) {
                                            log::info!("devices: {e}");
                                        }
                                    });
                                }
                                Ok(LinkReport::Hello(_)) => {}
                                Err(e) => {
                                    log::debug!("link: not a report ({e}): {text}");
                                    // A command this server cannot read, from a newer
                                    // device: refused, so its sender is not left waiting.
                                    if let Some((to, ack)) = koan_core::remote::acks::unread_relay(&text) {
                                        answer_once(&username, &device, &to, ack)(
                                            koan_core::remote::acks::unreadable(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
    let _ = tokio::task::spawn_blocking(move || registry.unregister(&id)).await;
    log::info!("link: {name} ({platform}) unlinked for {username}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    /// Link `device` at `url` and read until the server sends device keys.
    fn keys_sent(url: String) -> Vec<koan_core::remote::link::LinkDeviceKey> {
        let (mut socket, _) = tungstenite::connect(url.as_str()).unwrap();
        if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = socket.get_ref() {
            tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        }
        loop {
            let tungstenite::Message::Text(text) = socket.read().unwrap() else {
                continue;
            };
            if let Ok(koan_core::remote::link::LinkCommand::DeviceKeys { keys }) =
                serde_json::from_str(&text)
            {
                return keys;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_key_is_kept_on_its_api_key_and_published_to_the_account() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let (state, _dir) = test_state();
        // Not `mate`: account changes are announced by user id process-wide,
        // and another test changes `mate`'s while its link is open.
        let key = api_key(&state, "owner");
        let pool = state.pool.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, build_test_router(state)).await });
        let public = b64.encode([1u8; 32]);
        let link = |auth: &str, device: &str, public: &str| {
            let public = public
                .replace('+', "%2B")
                .replace('/', "%2F")
                .replace('=', "%3D");
            format!(
                "ws://{addr}/rest/koanLink?{auth}&v=1.16.1&c=test&device={device}&deviceKey={public}"
            )
        };

        // Signed in with its key: kept, and the device hears its own.
        let url = link(&format!("apiKey={key}"), "dev-keyed", &public);
        let keys = tokio::task::spawn_blocking(move || keys_sent(url))
            .await
            .unwrap();
        assert!(
            keys.iter()
                .any(|k| k.id == "dev-keyed" && k.key == public && k.owner.is_none()),
            "{keys:?}"
        );

        // Signed in with the password, or sending what is not a key: nothing
        // is kept, though the link still hears the account's keys.
        let other = b64.encode([2u8; 32]);
        let url = link("u=owner&p=sesame", "dev-password", &other);
        let keys = tokio::task::spawn_blocking(move || keys_sent(url))
            .await
            .unwrap();
        assert!(keys.iter().all(|k| k.id != "dev-password"), "{keys:?}");
        let url = link(&format!("apiKey={key}"), "dev-bad", "not-a-key");
        let keys = tokio::task::spawn_blocking(move || keys_sent(url))
            .await
            .unwrap();
        assert!(keys.iter().all(|k| k.key != "not-a-key"), "{keys:?}");

        // Revoking the API key takes the public key with it.
        let db = pool.get().unwrap();
        db.conn.execute("DELETE FROM api_keys", []).unwrap();
        assert!(
            queries::api_keys::device_keys(&db.conn, "owner")
                .unwrap()
                .is_empty()
        );
    }

    /// jo's phone, shared with kim, is revoked while kim's device is linked:
    /// kim's device is sent a list without it, though neither the phone nor
    /// any device of kim's links again.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_revoked_shared_device_leaves_its_grantees_list() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let (state, _dir) = test_state();
        let pool = state.pool.clone();
        // Accounts of this test's own: account changes are announced by user
        // id process-wide.
        let raw = {
            let db = pool.get().unwrap();
            queries::auth::create_user(&db.conn, "keyowner", "pw-owner", Role::User).unwrap();
            queries::auth::create_user(&db.conn, "keygrantee", "pw-grantee", Role::User).unwrap();
            db.conn
                .execute(
                    "INSERT INTO link_grants (device, owner, grantee, created_at)
                     VALUES ('kdev-phone', 'keyowner', 'keygrantee', 0)",
                    [],
                )
                .unwrap();
            let owner = queries::auth::get_user_by_username(&db.conn, "keyowner")
                .unwrap()
                .unwrap();
            queries::api_keys::create_api_key(&db.conn, owner.id, "phone")
                .unwrap()
                .1
        };
        crate::clients::registry()
            .share("keyowner", "kdev-phone", "keygrantee", true)
            .unwrap();
        spawn_key_publisher(pool.clone(), Default::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, build_test_router(state)).await });
        let encode = |k: String| {
            k.replace('+', "%2B")
                .replace('/', "%2F")
                .replace('=', "%3D")
        };
        let phone_key = b64.encode([3u8; 32]);
        let url = format!(
            "ws://{addr}/rest/koanLink?apiKey={raw}&v=1.16.1&c=test&device=kdev-phone&deviceKey={}",
            encode(phone_key.clone())
        );
        tokio::task::spawn_blocking(move || keys_sent(url))
            .await
            .unwrap();

        let url = format!(
            "ws://{addr}/rest/koanLink?u=keygrantee&p=pw-grantee&v=1.16.1&c=test&device=kdev-grantee&deviceKey={}",
            encode(b64.encode([4u8; 32]))
        );
        let (revoke_tx, revoke_rx) = std::sync::mpsc::channel::<()>();
        let grantee = tokio::task::spawn_blocking(move || {
            let (mut socket, _) = tungstenite::connect(url.as_str()).unwrap();
            if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = socket.get_ref() {
                tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            }
            let mut saw_phone = false;
            loop {
                let tungstenite::Message::Text(text) = socket.read().unwrap() else {
                    continue;
                };
                let Ok(koan_core::remote::link::LinkCommand::DeviceKeys { keys }) =
                    serde_json::from_str(&text)
                else {
                    continue;
                };
                let has_phone = keys
                    .iter()
                    .any(|k| k.id == "kdev-phone" && k.owner.as_deref() == Some("keyowner"));
                if !saw_phone {
                    assert!(has_phone, "shared before the revocation: {keys:?}");
                    saw_phone = true;
                    revoke_tx.send(()).unwrap();
                } else if !has_phone {
                    return true;
                }
            }
        });
        tokio::task::spawn_blocking(move || {
            revoke_rx.recv().unwrap();
            let db = pool.get().unwrap();
            assert!(queries::api_keys::revoke_api_key_value(&db.conn, &raw).unwrap());
        })
        .await
        .unwrap();
        assert!(grantee.await.unwrap());
    }

    #[tokio::test]
    async fn device_keys_are_never_relayed() {
        let reg = crate::clients::registry();
        let forged = koan_core::remote::link::LinkCommand::DeviceKeys {
            keys: vec![koan_core::remote::link::LinkDeviceKey {
                id: "evil".into(),
                key: "AAAA".into(),
                owner: None,
            }],
        };
        assert!(
            reg.relay_from("relay-jo", Some("dev-a"), "dev-b", forged.clone())
                .is_err()
        );
        assert!(reg.relay("relay-jo", "dev-b", forged).is_err());

        // And over HTTP, as any credential for the account could send it.
        let (state, _dir) = test_state();
        let command = r#"{"type":"deviceKeys","keys":[{"id":"evil","key":"AAAA"}]}"#;
        let command: String = form_urlencoded::byte_serialize(command.as_bytes()).collect();
        let (_, body) = get_response(
            build_test_router(state),
            &format!(
                "/rest/koanCommand?u=owner&p=sesame&v=1.16.1&c=test&f=json&to=dev-b&command={command}"
            ),
        )
        .await;
        assert!(body.contains("\"failed\""), "{body}");
    }

    #[tokio::test]
    async fn the_server_lists_device_keys() {
        let (state, _dir) = test_state();
        let (_, body) = get_response(
            build_test_router(state),
            "/rest/getOpenSubsonicExtensions?f=json",
        )
        .await;
        assert!(body.contains("koanDeviceKeys"), "{body}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_link_closes_when_its_account_changes() {
        let (state, _dir) = test_state();
        let pool = state.pool.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, build_test_router(state)).await });
        let url = format!(
            "ws://{addr}/rest/koanLink?u=mate&p=hunter22&v=1.16.1&c=test&device=account-change-test"
        );
        let mut socket = tokio::task::spawn_blocking(move || {
            let (socket, _) = tungstenite::connect(url.as_str()).unwrap();
            if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = socket.get_ref() {
                tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            }
            socket
        })
        .await
        .unwrap();
        tokio::task::spawn_blocking(move || {
            let db = pool.get().unwrap();
            queries::auth::update_role(&db.conn, "mate", Role::User).unwrap();
        })
        .await
        .unwrap();
        let closed = tokio::task::spawn_blocking(move || {
            loop {
                match socket.read() {
                    Ok(tungstenite::Message::Close(_)) => return true,
                    Ok(_) => continue,
                    Err(_) => return false,
                }
            }
        })
        .await
        .unwrap();
        assert!(closed, "the link outlived a change to its account");
    }

    /// Commands that waited for a device go down its link in full and in
    /// order, however many there are, and the link stays up after them.
    #[tokio::test]
    async fn a_long_outbox_arrives_whole_and_in_order() {
        use koan_core::remote::acks::Envelope;
        use koan_core::remote::link::LinkCommand;
        const WAITING: usize = 1500;
        for i in 0..WAITING {
            crate::clients::seed_outbox(
                "drainer",
                "outbox-drain-test",
                Envelope::from(LinkCommand::JumpTo {
                    track_id: i.to_string(),
                }),
            );
        }
        let (state, _dir) = test_state();
        // Its own account: links are registered process-wide, and other tests
        // here end or refuse the links of the shared one.
        {
            let db = state.pool.get().unwrap();
            queries::auth::create_user(&db.conn, "drainer", "hunter22", Role::Readonly).unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, build_test_router(state)).await });
        let url = format!(
            "ws://{addr}/rest/koanLink?u=drainer&p=hunter22&v=1.16.1&c=test&device=outbox-drain-test"
        );
        let got = tokio::task::spawn_blocking(move || {
            let (mut socket, _) = tungstenite::connect(url.as_str()).unwrap();
            if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = socket.get_ref() {
                tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            }
            let mut got = Vec::new();
            while got.len() < WAITING {
                match socket.read() {
                    Ok(tungstenite::Message::Text(text)) => {
                        if let Ok(Envelope {
                            command: LinkCommand::JumpTo { track_id },
                            ..
                        }) = serde_json::from_str(&text)
                        {
                            got.push(track_id);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => panic!("the link ended after {} commands: {e}", got.len()),
                }
            }
            (got, socket)
        })
        .await
        .unwrap();
        let (got, _socket) = got;
        let expected: Vec<String> = (0..WAITING).map(|i| i.to_string()).collect();
        assert_eq!(got, expected);
        assert!(
            crate::clients::registry()
                .list(Some("drainer"))
                .iter()
                .any(|c| c.device == "outbox-drain-test"),
            "the link survived its outbox"
        );
    }
}

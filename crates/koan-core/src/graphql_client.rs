//! GraphQL client for connecting to a `koan serve` instance.
//!
//! Uses blocking reqwest — call from a background thread when used from the TUI.

use std::sync::Arc;

use parking_lot::Mutex;
use reqwest::StatusCode;
use serde_json::Value;

use crate::config::Config;

/// A GraphQL client that talks to a koan server.
///
/// Clones share one session, so a token refreshed by one thread is the token
/// every other thread uses next.
#[derive(Clone)]
pub struct GraphQLClient {
    url: String,
    http: reqwest::blocking::Client,
    session: Option<Arc<Session>>,
    /// Cleared once the server turns out to predate play modes, after which
    /// `nowPlaying` is asked for without them. Shared by clones, like the
    /// session.
    play_modes: Arc<std::sync::atomic::AtomicBool>,
    /// The same for the sleep timer.
    sleep_timer: Arc<std::sync::atomic::AtomicBool>,
    /// The same for `moving`.
    moving: Arc<std::sync::atomic::AtomicBool>,
}

/// A sign-in to a server with auth enabled: the refresh token `koan auth login`
/// stored, and the access token it last bought.
struct Session {
    tokens: Mutex<Tokens>,
    /// Called with each new refresh token. The server spends the old one on
    /// every refresh, so a copy not written back is dead.
    on_rotate: Box<dyn Fn(&str) + Send + Sync>,
}

struct Tokens {
    access: Option<String>,
    refresh: String,
    /// Why the server refused the refresh token. Set once and kept: the token
    /// will not become valid again, and the bridge reconnects whenever the
    /// connection drops.
    refused: Option<String>,
}

impl GraphQLClient {
    pub fn new(server_url: &str) -> Self {
        let url = format!("{}/graphql", server_url.trim_end_matches('/'));
        Self {
            url,
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
            session: None,
            play_modes: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            sleep_timer: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            moving: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    /// A client carrying the sign-in stored in `[auth]`, when it is for this
    /// server. Rotated refresh tokens are written back through `Config::persist`.
    pub fn from_config(server_url: &str) -> Self {
        let client = Self::new(server_url);
        let cfg = Config::load().unwrap_or_default();
        if cfg.auth.refresh_token.is_empty()
            || cfg.auth.server.trim_end_matches('/') != client.server_url()
        {
            return client;
        }
        client.with_session(cfg.auth.refresh_token, |token| {
            if let Err(e) = Config::persist(|cfg| cfg.auth.refresh_token = token.to_owned()) {
                log::warn!("could not store the rotated refresh token: {e}");
            }
        })
    }

    /// Authenticate with `refresh_token`, exchanging it for an access token
    /// when the server first refuses a request.
    pub fn with_session(
        mut self,
        refresh_token: impl Into<String>,
        on_rotate: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        self.session = Some(Arc::new(Session {
            tokens: Mutex::new(Tokens {
                access: None,
                refresh: refresh_token.into(),
                refused: None,
            }),
            on_rotate: Box::new(on_rotate),
        }));
        self
    }

    /// Whether requests carry a stored sign-in.
    pub fn has_session(&self) -> bool {
        self.session.is_some()
    }

    /// Execute a raw GraphQL query/mutation.
    ///
    /// Access tokens are short-lived, so a 401 is answered by one refresh and
    /// one retry; a second refusal is `GraphQLError::Unauthorized`.
    pub fn execute(&self, query: &str, variables: Option<Value>) -> Result<Value, GraphQLError> {
        let mut body = serde_json::json!({ "query": query });
        if let Some(vars) = variables {
            body["variables"] = vars;
        }

        let access = self
            .session
            .as_ref()
            .and_then(|s| s.tokens.lock().access.clone());
        let mut resp = self.post(&body, access.as_deref())?;
        if resp.status() == StatusCode::UNAUTHORIZED
            && let Some(session) = &self.session
        {
            let fresh = self.refresh(session, access.as_deref())?;
            resp = self.post(&body, Some(&fresh))?;
        }

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(GraphQLError::Unauthorized(reason(resp)));
        }
        if !status.is_success() {
            return Err(GraphQLError::Http(format!("{status}: {}", reason(resp))));
        }

        let resp: Value = resp.json().map_err(|e| GraphQLError::Http(e.to_string()))?;

        if let Some(errors) = resp.get("errors")
            && let Some(arr) = errors.as_array()
            && !arr.is_empty()
        {
            let msg = arr[0]
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(GraphQLError::Query(msg.to_string()));
        }

        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    }

    fn post(
        &self,
        body: &Value,
        access: Option<&str>,
    ) -> Result<reqwest::blocking::Response, GraphQLError> {
        let mut req = self.http.post(&self.url).json(body);
        if let Some(token) = access {
            req = req.bearer_auth(token);
        }
        req.send().map_err(|e| GraphQLError::Http(e.to_string()))
    }

    /// A new access token, replacing `stale`.
    ///
    /// The lock is held across the request: the server spends a refresh token
    /// on use, so two threads refreshing at once would leave one holding a
    /// revoked token. A thread that waited finds `stale` already replaced and
    /// takes the new token instead.
    fn refresh(&self, session: &Session, stale: Option<&str>) -> Result<String, GraphQLError> {
        let mut tokens = session.tokens.lock();
        if let Some(access) = &tokens.access
            && Some(access.as_str()) != stale
        {
            return Ok(access.clone());
        }
        if let Some(reason) = &tokens.refused {
            return Err(GraphQLError::Unauthorized(reason.clone()));
        }

        let resp = self
            .http
            .post(format!("{}/auth/refresh", self.server_url()))
            .json(&serde_json::json!({ "refresh_token": tokens.refresh }))
            .send()
            .map_err(|e| GraphQLError::Http(e.to_string()))?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            let refused = format!("the stored sign-in was refused: {}", reason(resp));
            tokens.access = None;
            tokens.refused = Some(refused.clone());
            return Err(GraphQLError::Unauthorized(refused));
        }
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(GraphQLError::Http(format!(
                "/auth/refresh {status}: {}",
                reason(resp)
            )));
        }

        let body: Value = resp.json().map_err(|e| GraphQLError::Http(e.to_string()))?;
        let (Some(access), Some(refresh)) = (
            body["access_token"].as_str(),
            body["refresh_token"].as_str(),
        ) else {
            return Err(GraphQLError::Http(
                "malformed /auth/refresh response".into(),
            ));
        };
        tokens.access = Some(access.to_owned());
        tokens.refresh = refresh.to_owned();
        (session.on_rotate)(refresh);
        Ok(access.to_owned())
    }

    // -----------------------------------------------------------------------
    // Typed helpers
    // -----------------------------------------------------------------------

    /// The `nowPlaying` selection this server understands. A server older
    /// than play modes or the sleep timer rejects a query naming them; see
    /// [`GraphQLClient::predates`].
    fn now_playing_fields(&self) -> String {
        use std::sync::atomic::Ordering;
        const TRACK: &str = "track { trackId title artist album codec sampleRate bitDepth \
                             bitrateKbps channels durationMs }";
        let modes = if self.play_modes.load(Ordering::Relaxed) {
            "shuffle repeat "
        } else {
            ""
        };
        let sleep = if self.sleep_timer.load(Ordering::Relaxed) {
            "sleep { endsAtMs endOf } "
        } else {
            ""
        };
        let moving = if self.moving.load(Ordering::Relaxed) {
            "moving "
        } else {
            ""
        };
        format!("state positionMs durationMs queueItemId {moving}{modes}{sleep}{TRACK}")
    }

    /// Whether `error` is a server refusing a `nowPlaying` field it predates.
    /// If so, the field is left out from then on and reads as off, and the
    /// caller asks again.
    pub fn predates(&self, error: &GraphQLError) -> bool {
        use std::sync::atomic::Ordering;
        let GraphQLError::Query(e) = error else {
            return false;
        };
        if e.contains("moving") && self.moving.swap(false, Ordering::Relaxed) {
            log::info!("server predates moving: {e}");
            return true;
        }
        if e.contains("sleep") && self.sleep_timer.swap(false, Ordering::Relaxed) {
            log::info!("server predates the sleep timer: {e}");
            return true;
        }
        if (e.contains("shuffle") || e.contains("repeat"))
            && self.play_modes.swap(false, Ordering::Relaxed)
        {
            log::info!("server predates play modes: {e}");
            return true;
        }
        false
    }

    /// What the server is playing.
    pub fn now_playing(&self) -> Result<NowPlaying, GraphQLError> {
        loop {
            match self.execute(
                &format!("{{ nowPlaying {{ {} }} }}", self.now_playing_fields()),
                None,
            ) {
                Err(e) if self.predates(&e) => {}
                other => return Ok(NowPlaying::from_json(&other?["nowPlaying"])),
            }
        }
    }

    /// The `nowPlaying` subscription, for [`GraphQLClient::subscribe`]. Its
    /// messages carry the same object [`NowPlaying::from_json`] reads.
    pub fn now_playing_subscription(&self) -> String {
        format!(
            "subscription {{ nowPlaying {{ {} }} }}",
            self.now_playing_fields()
        )
    }

    pub fn queue(&self) -> Result<Vec<QueueEntry>, GraphQLError> {
        let data = self.execute(&format!("{{ queue {{ {QUEUE_FIELDS} }} }}"), None)?;
        Ok(QueueEntry::list(&data["queue"]))
    }

    /// The `queueUpdated` subscription, for [`GraphQLClient::subscribe`]. Its
    /// messages' `queueUpdated.entries` is what [`QueueEntry::list`] reads.
    pub fn queue_subscription() -> String {
        format!("subscription {{ queueUpdated {{ entries {{ {QUEUE_FIELDS} }} }} }}")
    }

    /// Follow `queries`, each a GraphQL subscription, over `/graphql/ws` in the
    /// `graphql-transport-ws` protocol, handing every message's `data` to
    /// `on_next` with the index of the query it answers.
    ///
    /// Blocks while the connection lasts and sleeps between messages, pinging
    /// only after a long silence so a dead connection is noticed. Returns why
    /// it ended, which it always does with an error: these subscriptions do not
    /// complete by themselves. The handshake carries the session's access
    /// token, and is refreshed and tried once more if the server refuses it.
    pub fn subscribe(
        &self,
        queries: &[String],
        on_next: impl FnMut(usize, &Value),
    ) -> GraphQLError {
        let (mut socket, fd) = match self.open_socket() {
            Ok(opened) => opened,
            Err(e) => return e,
        };
        let waker = match crate::remote::wire::Waker::new() {
            Ok(waker) => waker,
            Err(e) => return GraphQLError::Http(e.to_string()),
        };
        let mut session = Subscriptions {
            queries,
            on_next,
            stage: Stage::Init,
            pongs: 0,
            ended: None,
        };
        match crate::remote::wire::drive(&mut socket, fd, &waker, &mut session) {
            Ok(()) => session
                .ended
                .unwrap_or_else(|| GraphQLError::Http("the subscription ended".into())),
            Err(e) => GraphQLError::Http(e),
        }
    }

    fn open_socket(&self) -> Result<(WsSocket, std::os::fd::RawFd), GraphQLError> {
        let url = match self.url.split_once("://") {
            Some(("https", rest)) => format!("wss://{rest}/ws"),
            Some(("http", rest)) => format!("ws://{rest}/ws"),
            _ => {
                return Err(GraphQLError::Http(format!(
                    "not an http(s) server: {}",
                    self.url
                )));
            }
        };
        let access = self
            .session
            .as_ref()
            .and_then(|s| s.tokens.lock().access.clone());
        let socket = match (handshake(&url, access.as_deref()), &self.session) {
            (Err(GraphQLError::Unauthorized(_)), Some(session)) => {
                let fresh = self.refresh(session, access.as_deref())?;
                handshake(&url, Some(&fresh))?
            }
            (other, _) => other?,
        };
        let fd = crate::remote::wire::prepare(socket.get_ref()).map_err(GraphQLError::Http)?;
        Ok((socket, fd))
    }

    // -- Mutations --

    pub fn pause(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { pause { ok } }", None)?;
        Ok(())
    }

    pub fn resume(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { resume { ok } }", None)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { stop { ok } }", None)?;
        Ok(())
    }

    pub fn next(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { next { ok } }", None)?;
        Ok(())
    }

    pub fn previous(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { previous { ok } }", None)?;
        Ok(())
    }

    pub fn set_shuffle(&self, on: bool) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($on: Boolean!) { setPlayMode(shuffle: $on) { ok } }",
            Some(serde_json::json!({ "on": on })),
        )?;
        Ok(())
    }

    /// Set the server's sleep timer, or with `None` cancel it.
    pub fn set_sleep_timer(
        &self,
        timer: Option<crate::player::state::SleepTimer>,
    ) -> Result<(), GraphQLError> {
        use crate::player::state::SleepTimer;
        let (query, vars) = match timer {
            None => ("mutation { cancelSleepTimer { ok } }", None),
            Some(SleepTimer::After { minutes }) => (
                "mutation($m: Int!) { setSleepTimer(minutes: $m) { ok } }",
                Some(serde_json::json!({ "m": minutes })),
            ),
            Some(SleepTimer::EndOfTrack) => {
                ("mutation { setSleepTimer(endOf: TRACK) { ok } }", None)
            }
            Some(SleepTimer::EndOfRecord) => {
                ("mutation { setSleepTimer(endOf: RECORD) { ok } }", None)
            }
        };
        self.execute(query, vars)?;
        Ok(())
    }

    pub fn set_repeat(&self, repeat: crate::player::state::Repeat) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($repeat: Repeat!) { setPlayMode(repeat: $repeat) { ok } }",
            Some(serde_json::json!({ "repeat": repeat.as_str().to_uppercase() })),
        )?;
        Ok(())
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($positionMs: Int!) { seek(positionMs: $positionMs) { ok } }",
            Some(serde_json::json!({ "positionMs": position_ms })),
        )?;
        Ok(())
    }

    pub fn play(&self, queue_item_id: &str) -> Result<(), GraphQLError> {
        self.execute(
            "mutation($queueItemId: String!) { play(queueItemId: $queueItemId) { ok } }",
            Some(serde_json::json!({ "queueItemId": queue_item_id })),
        )?;
        Ok(())
    }

    pub fn clear_queue(&self) -> Result<(), GraphQLError> {
        self.execute("mutation { clearQueue { ok } }", None)?;
        Ok(())
    }

    pub fn library_stats(&self) -> Result<Value, GraphQLError> {
        self.execute(
            "{ libraryStats { totalTracks totalArtists totalAlbums localTracks remoteTracks cachedTracks } }",
            None,
        )
    }

    /// Server URL (without /graphql path).
    pub fn server_url(&self) -> &str {
        self.url.trim_end_matches("/graphql")
    }
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum GraphQLError {
    #[error("http error: {0}")]
    Http(String),
    #[error("query error: {0}")]
    Query(String),
    #[error("unauthorised: {0}")]
    Unauthorized(String),
}

/// What the server said when it refused a request: the `message` of a JSON
/// body, or the body as text.
fn reason(resp: reqwest::blocking::Response) -> String {
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_owned))
        .unwrap_or(text);
    if message.trim().is_empty() {
        status.to_string()
    } else {
        message.trim().to_owned()
    }
}

const QUEUE_FIELDS: &str =
    "queueItemId trackId title artist album codec trackNumber disc durationMs isCurrent";

type WsSocket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

/// How long opening the subscription socket may take, connect and upgrade
/// each: the same bound the queries have.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Certificates checked against the platform's roots, as reqwest checks them
/// for the queries.
fn tls() -> Result<Arc<rustls::ClientConfig>, GraphQLError> {
    use rustls_platform_verifier::ConfigVerifierExt;
    static CONFIG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    if let Some(config) = CONFIG.get() {
        return Ok(config.clone());
    }
    let config = Arc::new(
        rustls::ClientConfig::with_platform_verifier()
            .map_err(|e| GraphQLError::Http(format!("TLS: {e}")))?,
    );
    Ok(CONFIG.get_or_init(|| config).clone())
}

/// Open `url` as a `graphql-transport-ws` socket, signed in with `access`.
/// Bounded like a query: a server that takes the connection and never
/// answers the upgrade fails the attempt rather than holding it.
fn handshake(url: &str, access: Option<&str>) -> Result<WsSocket, GraphQLError> {
    use std::net::ToSocketAddrs;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::handshake::HandshakeError;
    use tungstenite::http::HeaderValue;
    let http = |e: &dyn std::fmt::Display| GraphQLError::Http(e.to_string());

    let mut request = url.into_client_request().map_err(|e| http(&e))?;
    let headers = request.headers_mut();
    headers.insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("graphql-transport-ws"),
    );
    if let Some(token) = access {
        let bearer = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| http(&e))?;
        headers.insert("Authorization", bearer);
    }

    let uri = request.uri();
    let secure = uri.scheme_str() == Some("wss");
    let host = uri
        .host()
        .ok_or_else(|| http(&"no host in the server's address"))?;
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let mut last = None;
    let mut stream = None;
    for addr in (host.trim_matches(['[', ']']), port)
        .to_socket_addrs()
        .map_err(|e| http(&e))?
    {
        match std::net::TcpStream::connect_timeout(&addr, HANDSHAKE_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let stream = stream.ok_or_else(|| match last {
        Some(e) => http(&e),
        None => http(&format!("{host} has no address")),
    })?;
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)))
        .map_err(|e| http(&e))?;
    let connector = if secure {
        tungstenite::Connector::Rustls(tls()?)
    } else {
        tungstenite::Connector::Plain
    };

    match tungstenite::client_tls_with_config(request, stream, None, Some(connector)) {
        Ok((socket, _)) => Ok(socket),
        Err(HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            let status = response.status();
            let body = response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).trim().to_owned())
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| status.to_string());
            if status == StatusCode::UNAUTHORIZED {
                Err(GraphQLError::Unauthorized(body))
            } else {
                Err(GraphQLError::Http(format!("{status}: {body}")))
            }
        }
        // A read that timed out leaves the handshake part way.
        Err(HandshakeError::Interrupted(_)) => Err(http(&"the server did not answer the upgrade")),
        Err(HandshakeError::Failure(e)) => Err(http(&e)),
    }
}

/// Where a `graphql-transport-ws` conversation has got to.
#[derive(PartialEq)]
enum Stage {
    /// `connection_init` is still to be sent.
    Init,
    /// Sent; waiting for `connection_ack`.
    Waiting,
    /// Acknowledged; the subscriptions are still to be sent.
    Acked,
    Subscribed,
}

struct Subscriptions<'a, F> {
    queries: &'a [String],
    on_next: F,
    stage: Stage,
    /// Pings from the server still to be answered.
    pongs: usize,
    ended: Option<GraphQLError>,
}

impl<F: FnMut(usize, &Value)> crate::remote::wire::Session for Subscriptions<'_, F> {
    fn outgoing(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        match self.stage {
            Stage::Init => {
                out.push(serde_json::json!({ "type": "connection_init" }).to_string());
                self.stage = Stage::Waiting;
            }
            Stage::Acked => {
                for (id, query) in self.queries.iter().enumerate() {
                    out.push(
                        serde_json::json!({
                            "id": id.to_string(),
                            "type": "subscribe",
                            "payload": { "query": query },
                        })
                        .to_string(),
                    );
                }
                self.stage = Stage::Subscribed;
            }
            Stage::Waiting | Stage::Subscribed => {}
        }
        for _ in 0..std::mem::take(&mut self.pongs) {
            out.push(serde_json::json!({ "type": "pong" }).to_string());
        }
        out
    }

    fn incoming(&mut self, text: &str) {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let first_error = |errors: &Value| {
            errors[0]["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_owned()
        };
        match message["type"].as_str() {
            Some("connection_ack") if self.stage == Stage::Waiting => self.stage = Stage::Acked,
            Some("ping") => self.pongs += 1,
            Some("next") => {
                let payload = &message["payload"];
                if payload["errors"].as_array().is_some_and(|e| !e.is_empty()) {
                    self.ended = Some(GraphQLError::Query(first_error(&payload["errors"])));
                } else if let Some(id) = message["id"].as_str().and_then(|id| id.parse().ok()) {
                    (self.on_next)(id, &payload["data"]);
                }
            }
            Some("error") => {
                self.ended = Some(GraphQLError::Query(first_error(&message["payload"])));
            }
            Some("complete") => {
                self.ended = Some(GraphQLError::Http("the server ended a subscription".into()));
            }
            _ => {}
        }
    }

    fn done(&self) -> bool {
        self.ended.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub state: String,
    /// Whether the playhead is advancing by itself, which playing does not
    /// mean while the server waits on a download.
    pub moving: bool,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub queue_item_id: Option<String>,
    pub mode: crate::player::state::PlayMode,
    pub sleep: Option<crate::player::state::Sleep>,
    pub track: Option<NowPlayingTrack>,
}

impl NowPlaying {
    /// A `nowPlaying` object, as the query and the subscription both send it.
    pub fn from_json(np: &Value) -> Self {
        let state = np["state"].as_str().unwrap_or("STOPPED").to_string();
        Self {
            // A server that predates it held nothing back.
            moving: np["moving"].as_bool().unwrap_or(state == "PLAYING"),
            state,
            position_ms: np["positionMs"].as_u64().unwrap_or(0),
            duration_ms: np["durationMs"].as_u64(),
            queue_item_id: np["queueItemId"].as_str().map(String::from),
            mode: crate::player::state::PlayMode {
                shuffle: np["shuffle"].as_bool().unwrap_or(false),
                repeat: np["repeat"]
                    .as_str()
                    .and_then(|r| crate::player::state::Repeat::parse(&r.to_lowercase()))
                    .unwrap_or_default(),
            },
            sleep: match (
                np["sleep"]["endsAtMs"].as_u64(),
                np["sleep"]["endOf"].as_str(),
            ) {
                (Some(unix_ms), _) => Some(crate::player::state::Sleep::At { unix_ms }),
                (None, Some("TRACK")) => Some(crate::player::state::Sleep::EndOfTrack),
                (None, Some("RECORD")) => Some(crate::player::state::Sleep::EndOfRecord),
                _ => None,
            },
            track: np
                .get("track")
                .filter(|t| !t.is_null())
                .map(|t| NowPlayingTrack {
                    track_id: t["trackId"].as_str().map(String::from),
                    title: t["title"].as_str().unwrap_or("").to_string(),
                    artist: t["artist"].as_str().unwrap_or("").to_string(),
                    album: t["album"].as_str().unwrap_or("").to_string(),
                    codec: t["codec"].as_str().unwrap_or("").to_string(),
                    sample_rate: t["sampleRate"].as_u64().unwrap_or(0) as u32,
                    bit_depth: t["bitDepth"].as_u64().map(|v| v as u16),
                    bitrate_kbps: t["bitrateKbps"].as_u64().map(|v| v as u32),
                    channels: t["channels"].as_u64().unwrap_or(0) as u16,
                    duration_ms: t["durationMs"].as_u64().unwrap_or(0),
                }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NowPlayingTrack {
    /// The track's id on the server. `None` for a queue entry the server built
    /// from a file with no database row, which cannot be streamed.
    pub track_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: Option<u16>,
    pub bitrate_kbps: Option<u32>,
    pub channels: u16,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub queue_item_id: String,
    pub track_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
    pub is_current: bool,
}

impl QueueEntry {
    /// A list of queue entries, as the query and the subscription both send it.
    pub fn list(entries: &Value) -> Vec<Self> {
        let Some(entries) = entries.as_array() else {
            return Vec::new();
        };
        entries
            .iter()
            .map(|e| QueueEntry {
                queue_item_id: e["queueItemId"].as_str().unwrap_or("").to_string(),
                track_id: e["trackId"].as_str().map(String::from),
                title: e["title"].as_str().unwrap_or("").to_string(),
                artist: e["artist"].as_str().unwrap_or("").to_string(),
                album: e["album"].as_str().unwrap_or("").to_string(),
                codec: e["codec"].as_str().map(String::from),
                track_number: e["trackNumber"].as_i64(),
                disc: e["disc"].as_i64(),
                duration_ms: e["durationMs"].as_u64(),
                is_current: e["isCurrent"].as_bool().unwrap_or(false),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `graphql-transport-ws` conversation: init, then the subscriptions
    /// once acknowledged, pings answered, data handed on by query, and an
    /// error ending it.
    #[test]
    fn subscriptions_follow_the_protocol() {
        use crate::remote::wire::Session;
        let queries = [
            "subscription { a }".to_owned(),
            "subscription { b }".to_owned(),
        ];
        let mut seen = Vec::new();
        let mut session = Subscriptions {
            queries: &queries,
            on_next: |id: usize, data: &Value| seen.push((id, data.clone())),
            stage: Stage::Init,
            pongs: 0,
            ended: None,
        };
        let sent = |out: Vec<String>| -> Vec<Value> {
            out.iter()
                .map(|m| serde_json::from_str(m).unwrap())
                .collect()
        };

        let init = sent(session.outgoing());
        assert_eq!(init.len(), 1);
        assert_eq!(init[0]["type"], "connection_init");
        assert!(session.outgoing().is_empty(), "nothing until the ack");

        session.incoming(r#"{"type":"connection_ack"}"#);
        let subscribed = sent(session.outgoing());
        assert_eq!(subscribed.len(), 2);
        assert_eq!(subscribed[1]["id"], "1");
        assert_eq!(subscribed[1]["type"], "subscribe");
        assert_eq!(subscribed[1]["payload"]["query"], "subscription { b }");
        assert!(session.outgoing().is_empty(), "subscribed once");

        session.incoming(r#"{"type":"ping"}"#);
        assert_eq!(sent(session.outgoing())[0]["type"], "pong");

        session.incoming(r#"{"type":"next","id":"1","payload":{"data":{"b":2}}}"#);
        assert!(!session.done());
        session.incoming(
            r#"{"type":"error","id":"0","payload":[{"message":"Unknown field \"sleep\""}]}"#,
        );
        assert!(session.done());
        assert!(matches!(&session.ended, Some(GraphQLError::Query(m)) if m.contains("sleep")));
        drop(session);
        assert_eq!(seen, [(1, serde_json::json!({ "b": 2 }))]);
    }

    #[test]
    fn a_refused_field_is_left_out_from_then_on() {
        let c = GraphQLClient::new("http://localhost:4000");
        assert!(c.now_playing_subscription().contains("sleep {"));
        assert!(c.predates(&GraphQLError::Query("Unknown field \"sleep\"".into())));
        assert!(!c.now_playing_subscription().contains("sleep"));
        assert!(c.now_playing_subscription().contains("shuffle"));
        assert!(
            !c.predates(&GraphQLError::Query("Unknown field \"sleep\"".into())),
            "once"
        );
        assert!(!c.predates(&GraphQLError::Http("refused".into())));
    }

    #[test]
    fn client_constructs_url() {
        let c = GraphQLClient::new("http://localhost:4000");
        assert_eq!(c.url, "http://localhost:4000/graphql");
    }

    #[test]
    fn client_trailing_slash() {
        let c = GraphQLClient::new("http://localhost:4000/");
        assert_eq!(c.url, "http://localhost:4000/graphql");
    }

    /// A request as the test server saw it.
    struct Seen {
        path: String,
        bearer: Option<String>,
        body: String,
    }

    /// An HTTP server on a loopback port answering each request with
    /// `respond`. Returns its base URL and every request it received.
    fn serve(
        respond: impl Fn(&Seen) -> (u16, &'static str) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader, Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen_log = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                let (mut bearer, mut length) = (None, 0);
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let header = line.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    let (name, value) = header.split_once(": ").unwrap_or((header, ""));
                    match name.to_ascii_lowercase().as_str() {
                        "authorization" => {
                            bearer = value.strip_prefix("Bearer ").map(str::to_owned)
                        }
                        "content-length" => length = value.parse().unwrap_or(0),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let seen = Seen {
                    path,
                    bearer,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                seen_log.lock().push(seen.path.clone());
                let (code, reply) = respond(&seen);
                write!(
                    stream,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
        });
        (base, log)
    }

    /// A server that takes `access-2` and trades `refresh-1` for it once.
    fn signed_in_server(seen: &Seen) -> (u16, &'static str) {
        match seen.path.as_str() {
            "/graphql" if seen.bearer.as_deref() == Some("access-2") => {
                (200, r#"{"data":{"ok":true}}"#)
            }
            "/auth/refresh" if seen.body.contains("refresh-1") => (
                200,
                r#"{"access_token":"access-2","refresh_token":"refresh-2"}"#,
            ),
            "/auth/refresh" => (401, r#"{"message":"invalid or expired refresh token"}"#),
            _ => (401, "missing or invalid Authorization header"),
        }
    }

    #[test]
    fn refused_request_refreshes_once_and_retries() {
        let (base, log) = serve(signed_in_server);
        let rotated = Arc::new(Mutex::new(Vec::new()));
        let stored = rotated.clone();
        let client = GraphQLClient::new(&base)
            .with_session("refresh-1", move |t| stored.lock().push(t.to_owned()));

        let data = client.execute("{ ok }", None).unwrap();
        assert_eq!(data["ok"], true);
        assert_eq!(*rotated.lock(), ["refresh-2"]);

        // The new access token is kept: no second refresh.
        client.clone().execute("{ ok }", None).unwrap();
        assert_eq!(
            *log.lock(),
            ["/graphql", "/auth/refresh", "/graphql", "/graphql"]
        );
    }

    #[test]
    fn refused_refresh_is_unauthorised() {
        let (base, log) = serve(signed_in_server);
        let client = GraphQLClient::new(&base).with_session("revoked", |_| {});

        for _ in 0..2 {
            match client.execute("{ ok }", None) {
                Err(GraphQLError::Unauthorized(msg)) => {
                    assert!(msg.contains("invalid or expired refresh token"), "{msg}")
                }
                other => panic!("expected Unauthorized, got {other:?}"),
            }
        }
        // A refused refresh token is not offered again.
        assert_eq!(*log.lock(), ["/graphql", "/auth/refresh", "/graphql"]);
    }

    #[test]
    fn unauthorised_without_a_session_says_why() {
        let (base, log) = serve(signed_in_server);

        match GraphQLClient::new(&base).execute("{ ok }", None) {
            Err(GraphQLError::Unauthorized(msg)) => {
                assert_eq!(msg, "missing or invalid Authorization header")
            }
            other => panic!("expected Unauthorized, got {other:?}"),
        }
        assert_eq!(*log.lock(), ["/graphql"]);
    }
}

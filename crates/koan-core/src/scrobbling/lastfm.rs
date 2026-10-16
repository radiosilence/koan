//! Last.fm's web API (<https://www.last.fm/api>): the account connection round
//! trip and signed `track.scrobble` / `track.updateNowPlaying` calls.
//!
//! Last.fm identifies the application by an API key and shared secret the
//! server's admin registers (`[lastfm]` in the config), and each account by a
//! session key it earns by sending its browser to Last.fm and back. The
//! session key is kept where ListenBrainz's token is, in `scrobble_services`,
//! and like it never leaves the server.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;

use super::{Failure, Listen, Sent};
use crate::auth::now_unix;
use crate::config::Config;
use crate::db::queries::scrobbling::{self as queries, LASTFM};

const API: &str = "https://ws.audioscrobbler.com/2.0/";
const AUTH: &str = "https://www.last.fm/api/auth/";

/// Scrobbles per `track.scrobble` call: the most Last.fm takes.
pub(super) const BATCH: usize = 50;

/// Last.fm ignores a scrobble older than two weeks. A margin keeps one that is
/// about to cross that line while it is sent from being counted as lost.
const WINDOW_SECS: i64 = 14 * 24 * 60 * 60 - 60 * 60;

/// How long a person has to approve kōan on Last.fm and come back.
const APPROVAL: Duration = Duration::from_secs(15 * 60);

/// Where Last.fm is reached in place of [`API`]: a stand-in for tests.
static API_OVERRIDE: Mutex<Option<String>> = Mutex::new(None);

fn api() -> String {
    API_OVERRIDE
        .lock()
        .clone()
        .unwrap_or_else(|| API.to_owned())
}

/// Send every Last.fm API call in this process to `base` instead. For tests,
/// which answer for Last.fm themselves.
#[doc(hidden)]
pub fn use_api(base: &str) {
    *API_OVERRIDE.lock() = Some(base.to_owned());
}

/// The earliest `played_at` Last.fm takes from `service`, for one it limits.
pub(super) fn oldest_taken(service: &str) -> Option<i64> {
    (service == LASTFM).then(|| now_unix() as i64 - WINDOW_SECS)
}

/// The server's Last.fm API key and shared secret.
#[derive(Clone)]
struct Credentials {
    api_key: String,
    secret: String,
}

/// An API key and secret in place of the config's: a stand-in for tests.
static CREDENTIALS_OVERRIDE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Use `api_key` and `secret` in this process instead of the config's. For
/// tests.
#[doc(hidden)]
pub fn use_credentials(api_key: &str, secret: &str) {
    *CREDENTIALS_OVERRIDE.lock() = Some((api_key.to_owned(), secret.to_owned()));
}

fn credentials() -> Option<Credentials> {
    if let Some((api_key, secret)) = CREDENTIALS_OVERRIDE.lock().clone() {
        return Some(Credentials { api_key, secret });
    }
    let cfg = Config::load().unwrap_or_default().lastfm;
    cfg.is_set().then(|| Credentials {
        api_key: cfg.api_key.trim().to_owned(),
        secret: cfg.secret.trim().to_owned(),
    })
}

/// Whether this server has been given an API key and secret.
pub fn configured() -> bool {
    credentials().is_some()
}

/// `api_sig`: the MD5 of every parameter but `format` and `callback`, in
/// order of name, as `namevalue` run together, with the shared secret on the
/// end.
fn sign(params: &[(String, String)], secret: &str) -> String {
    let mut sorted: Vec<&(String, String)> = params
        .iter()
        .filter(|(k, _)| k != "format" && k != "callback")
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut text = String::new();
    for (k, v) in sorted {
        text.push_str(k);
        text.push_str(v);
    }
    text.push_str(secret);
    format!("{:x}", md5::compute(text))
}

fn pair(k: impl Into<String>, v: impl Into<String>) -> (String, String) {
    (k.into(), v.into())
}

/// What Last.fm's error codes mean to a sender. The ones about this server's
/// key or signature are not the plays' fault, so they are waited out, not
/// dropped.
fn failure(code: i64, message: &str) -> Failure {
    match code {
        9 => Failure::Refused,
        // Service offline, temporarily unavailable, rate limited, or failed.
        8 | 11 | 16 | 29 => Failure::Unreachable(None),
        // Invalid API key, invalid signature, suspended key: the admin's to fix.
        10 | 13 | 26 => {
            log::warn!("scrobbling: Last.fm refuses this server's API key ({code}: {message})");
            Failure::Unreachable(None)
        }
        _ => Failure::Rejected(format!("{code}: {message}")),
    }
}

/// One signed call. `Ok` is Last.fm's answer; an `error` in it, whatever the
/// HTTP status, is a [`Failure`].
fn call(
    http: &reqwest::blocking::Client,
    creds: &Credentials,
    method: &str,
    mut params: Vec<(String, String)>,
) -> Result<Value, Failure> {
    params.push(pair("method", method));
    params.push(pair("api_key", &creds.api_key));
    let signature = sign(&params, &creds.secret);
    params.push(pair("api_sig", signature));
    params.push(pair("format", "json"));
    let resp = http.post(api()).form(&params).send().map_err(|e| {
        log::info!("scrobbling: Last.fm unreachable: {}", e.without_url());
        Failure::Unreachable(None)
    })?;
    let status = resp.status().as_u16();
    let body: Option<Value> = resp.json().ok();
    if let Some(code) = body.as_ref().and_then(|b| b["error"].as_i64()) {
        let message = body.as_ref().and_then(|b| b["message"].as_str());
        return Err(failure(code, message.unwrap_or("")));
    }
    match (status, body) {
        (200..=299, Some(body)) => Ok(body),
        (429, _) => Err(Failure::Unreachable(None)),
        (400..=499, _) => Err(Failure::Rejected(status.to_string())),
        _ => Err(Failure::Unreachable(None)),
    }
}

/// Add what Last.fm takes of `listen` to `params`, its parameter names
/// suffixed with `[index]` when sent in a batch.
fn track_params(params: &mut Vec<(String, String)>, listen: &Listen, index: Option<usize>) {
    let name = |k: &str| index.map_or_else(|| k.to_owned(), |i| format!("{k}[{i}]"));
    params.push(pair(name("artist"), &listen.artist));
    params.push(pair(name("track"), &listen.title));
    if let Some(album) = &listen.album {
        params.push(pair(name("album"), album));
    }
    if let Some(d) = listen.duration_ms.filter(|&d| d >= 1000) {
        params.push(pair(name("duration"), (d / 1000).to_string()));
    }
    if let Some(n) = listen.track_number.filter(|&n| n > 0) {
        params.push(pair(name("trackNumber"), n.to_string()));
    }
    if let Some(id) = &listen.recording_mbid {
        params.push(pair(name("mbid"), id));
    }
}

/// Scrobble `listens`, or with `now_playing` tell Last.fm the first is
/// playing.
pub(super) fn submit(
    http: &reqwest::blocking::Client,
    session_key: &str,
    now_playing: bool,
    listens: &[&Listen],
) -> Sent {
    let Some(creds) = credentials() else {
        log::warn!("scrobbling: Last.fm has no API key and secret on this server");
        return Err(Failure::Unreachable(None));
    };
    let mut params = vec![pair("sk", session_key)];
    if now_playing {
        let Some(listen) = listens.first() else {
            return Ok(());
        };
        track_params(&mut params, listen, None);
        call(http, &creds, "track.updateNowPlaying", params)?;
        return Ok(());
    }
    for (i, listen) in listens.iter().enumerate() {
        track_params(&mut params, listen, Some(i));
        params.push(pair(
            format!("timestamp[{i}]"),
            listen.played_at.to_string(),
        ));
    }
    let answer = call(http, &creds, "track.scrobble", params)?;
    let attr = &answer["scrobbles"]["@attr"]["ignored"];
    let ignored = attr
        .as_i64()
        .or_else(|| attr.as_str()?.parse().ok())
        .unwrap_or(0);
    if ignored > 0 {
        // Last.fm took the call and chose not to count these (a filtered
        // artist, a timestamp it finds wrong); sending them again changes
        // nothing.
        log::debug!(
            "scrobbling: Last.fm ignored {ignored} of {} scrobbles",
            listens.len()
        );
    }
    Ok(())
}

/// Why a Last.fm account could not be connected. Each message is fit to show
/// the person connecting.
#[derive(Debug, thiserror::Error)]
pub enum LastfmError {
    #[error("Last.fm is not set up on this server. Its admin needs to add a Last.fm API key.")]
    NotConfigured,
    #[error(
        "This server does not know where it is reached from outside, which Last.fm needs to send you back. Its admin needs to set sharing.public_url."
    )]
    NoPublicUrl,
    #[error("That Last.fm sign-in has expired or was not started here. Start again.")]
    Expired,
    #[error("Last.fm did not accept the approval. Start again.")]
    Declined,
    #[error("Last.fm could not be reached. Try again later.")]
    Unreachable,
    #[error("The connection could not be saved.")]
    Saving,
}

struct Attempt {
    user: i64,
    at: Instant,
}

/// Connections begun and not yet finished, by the one-time `state` in the
/// address Last.fm sends the person back to. Without it, a page that sent a
/// signed-in person to that address with the token of the page's own Last.fm
/// account would connect that account to theirs. Lost on restart, which only
/// asks the person to start again.
static ATTEMPTS: LazyLock<Mutex<HashMap<String, Attempt>>> = LazyLock::new(Default::default);

/// Begin connecting `user`'s Last.fm: the one-time state the return address
/// carries.
pub fn begin(user: i64) -> Result<String, LastfmError> {
    if !configured() {
        return Err(LastfmError::NotConfigured);
    }
    let state = crate::auth::random_token().map_err(|_| LastfmError::Unreachable)?;
    let mut attempts = ATTEMPTS.lock();
    attempts.retain(|_, a| a.at.elapsed() < APPROVAL);
    if attempts.len() >= 1000 {
        return Err(LastfmError::Unreachable);
    }
    attempts.insert(
        state.clone(),
        Attempt {
            user,
            at: Instant::now(),
        },
    );
    Ok(state)
}

/// Where to send the person to approve kōan, coming back to `callback`.
pub fn auth_url(callback: &str) -> Result<String, LastfmError> {
    let creds = credentials().ok_or(LastfmError::NotConfigured)?;
    let mut url = url::Url::parse(AUTH).map_err(|_| LastfmError::Unreachable)?;
    url.query_pairs_mut()
        .append_pair("api_key", &creds.api_key)
        .append_pair("cb", callback);
    Ok(url.into())
}

/// Whether `user` began a connection under `state`, still waiting. A state
/// serves once, whether or not it is right.
fn spend(user: i64, state: &str) -> bool {
    let begun = ATTEMPTS.lock().remove(state);
    begun.is_some_and(|a| a.user == user && a.at.elapsed() < APPROVAL)
}

/// Exchange the `token` Last.fm sent `user` back with, under the `state` they
/// were sent away with, for a session key and the account's name. Blocks on
/// the network, so call it without a database connection held.
pub fn session(user: i64, state: &str, token: &str) -> Result<(String, String), LastfmError> {
    let creds = credentials().ok_or(LastfmError::NotConfigured)?;
    if !spend(user, state) {
        return Err(LastfmError::Expired);
    }
    let token = token.trim();
    if token.is_empty() || token.len() > 200 {
        return Err(LastfmError::Declined);
    }
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| LastfmError::Unreachable)?;
    let answer = call(&http, &creds, "auth.getSession", vec![pair("token", token)]).map_err(
        |e| match e {
            Failure::Unreachable(_) => LastfmError::Unreachable,
            _ => LastfmError::Declined,
        },
    )?;
    match (
        answer["session"]["key"].as_str(),
        answer["session"]["name"].as_str(),
    ) {
        (Some(key), Some(name)) if !key.is_empty() => Ok((key.to_owned(), name.to_owned())),
        _ => Err(LastfmError::Declined),
    }
}

/// Connect `user`'s Last.fm with a session key [`session`] returned, queue the
/// part of their history Last.fm will still take and wake the sender. Returns
/// how many plays were queued, and the connection as stored.
pub fn connect(
    conn: &rusqlite::Connection,
    user: i64,
    session_key: &str,
    account_name: &str,
) -> Result<(usize, queries::ScrobbleService), LastfmError> {
    let since = oldest_taken(LASTFM).unwrap_or(0);
    let saved = queries::connect(conn, user, LASTFM, session_key, account_name, since)
        .and_then(|queued| Ok((queued, queries::service(conn, user, LASTFM)?)));
    match saved {
        Ok((queued, Some(service))) => {
            super::wake();
            Ok((queued, service))
        }
        Ok((_, None)) => Err(LastfmError::Saving),
        Err(e) => {
            log::warn!("scrobbling: saving a connection failed: {e}");
            Err(LastfmError::Saving)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    fn p(k: &str, v: &str) -> (String, String) {
        pair(k, v)
    }

    fn listen() -> Listen {
        Listen {
            played_at: 1_700_000_000,
            title: "Archangel".into(),
            artist: "Burial".into(),
            album: Some("Untrue".into()),
            duration_ms: Some(238_500),
            track_number: Some(2),
            recording_mbid: None,
            release_mbid: None,
        }
    }

    #[test]
    fn a_signature_covers_the_parameters_in_name_order_and_the_secret() {
        let params = vec![
            p("method", "auth.getSession"),
            p("format", "json"),
            p("api_key", "key"),
            p("token", "tok"),
            p("callback", "ignored"),
        ];
        let expected = format!(
            "{:x}",
            md5::compute("api_keykeymethodauth.getSessiontokentoksecret")
        );
        assert_eq!(sign(&params, "secret"), expected);
    }

    #[test]
    fn batch_parameters_are_indexed() {
        let mut params = Vec::new();
        track_params(&mut params, &listen(), Some(3));
        let names: Vec<&str> = params.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "artist[3]",
                "track[3]",
                "album[3]",
                "duration[3]",
                "trackNumber[3]"
            ]
        );
        assert_eq!(params[3].1, "238");
        let mut single = Vec::new();
        track_params(&mut single, &listen(), None);
        assert_eq!(single[0].0, "artist");
    }

    #[test]
    fn errors_are_waited_out_or_dropped_by_whose_fault_they_are() {
        assert!(matches!(failure(9, ""), Failure::Refused));
        assert!(matches!(failure(16, ""), Failure::Unreachable(None)));
        assert!(matches!(failure(29, ""), Failure::Unreachable(None)));
        assert!(matches!(failure(10, ""), Failure::Unreachable(None)));
        assert!(matches!(failure(6, "bad"), Failure::Rejected(_)));
    }

    #[test]
    fn only_lastfm_has_a_window() {
        assert!(oldest_taken(LASTFM).is_some());
        assert!(oldest_taken(queries::LISTENBRAINZ).is_none());
    }

    #[test]
    fn a_connection_must_return_the_state_it_left_with() {
        let attempt = |user| Attempt {
            user,
            at: Instant::now(),
        };
        ATTEMPTS.lock().insert("s1".into(), attempt(1));
        assert!(!spend(2, "s1"), "another account's");
        assert!(!spend(1, "s1"), "and spent by the try");
        ATTEMPTS.lock().insert("s2".into(), attempt(1));
        assert!(spend(1, "s2"));
        assert!(!spend(1, "s2"), "only once");
        assert!(!spend(1, "never-begun"));
    }

    /// Answer one request per connection with `reply(request body)`, and keep
    /// each body for the test to read.
    fn fake_lastfm(
        reply: impl Fn(&str) -> (&'static str, String) + Send + 'static,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/2.0/", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let (status, answer) = reply(&body);
                let _ = tx.send(body);
                let _ = write!(
                    &stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        (base, rx)
    }

    fn creds() -> Credentials {
        Credentials {
            api_key: "key".into(),
            secret: "secret".into(),
        }
    }

    fn client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::new()
    }

    #[test]
    fn a_call_is_signed_and_an_error_answer_is_a_failure() {
        let (base, rx) = fake_lastfm(|_| {
            (
                "403 Forbidden",
                r#"{"error":9,"message":"Invalid session key"}"#.into(),
            )
        });
        use_api(&base);
        let result = call(&client(), &creds(), "track.scrobble", vec![p("sk", "sess")]);
        assert!(matches!(result, Err(Failure::Refused)));
        let sent = rx.recv().unwrap();
        let expected = sign(
            &[
                p("sk", "sess"),
                p("method", "track.scrobble"),
                p("api_key", "key"),
            ],
            "secret",
        );
        assert!(sent.contains(&format!("api_sig={expected}")), "{sent}");
        assert!(
            sent.contains("format=json") && sent.contains("sk=sess"),
            "{sent}"
        );
        assert!(!sent.contains("secret"), "{sent}");
    }
}

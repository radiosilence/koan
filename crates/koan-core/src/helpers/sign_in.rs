//! Signing in to the remote server, the credentials kept for it, and the client built from them.

use std::sync::Arc;

use crate::config::Config;
use crate::db::connection::Database;
use crate::remote::client::{Credential, SubsonicAuth, SubsonicClient, SubsonicError};

use super::*;

/// What signs in to the remote server, from `config.local.toml` or a
/// `KOAN_REMOTE__*` variable layered over it: the API key when there is one,
/// else the password.
pub fn remote_credential(cfg: &Config) -> Option<Credential> {
    if !cfg.remote.api_key.is_empty() {
        return Some(Credential::ApiKey(cfg.remote.api_key.clone()));
    }
    (!cfg.remote.password.is_empty()).then(|| Credential::Password(cfg.remote.password.clone()))
}

/// Why signing in to a remote server failed.
#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error("the server did not accept those credentials: {0}")]
    Rejected(#[from] crate::remote::client::SubsonicError),
    /// Subsonic's error 41: the server cannot check the token a client signs
    /// with over plain HTTP against the account's password, and wants an app
    /// password or an API key instead.
    #[error(
        "this server needs an app password or API key: make one in the server's web UI and sign in with it"
    )]
    NeedsKey,
    #[error("could not write the configuration: {0}")]
    Config(#[from] crate::config::ConfigError),
}

/// Sign in to a Subsonic server with a password and remember it.
///
/// A koan server that offers `profile::SIGN_IN` is sent the password once and
/// answers with an API key for this device, which is what is kept, as joining
/// with an invite keeps one: the password never goes over the wire again, and
/// the key can be revoked on its own. An app password or the shared secret is
/// refused that trade, and is kept as typed. Any other server keeps the password in
/// `config.local.toml`, gitignored and written `0600`; Subsonic signs every
/// request with it or a salted MD5 of it, so what is kept is
/// password-equivalent wherever it is kept.
///
/// The credentials are checked against the server before anything is written; a
/// stored password that does not work is worse than none.
///
/// Shared by the CLI and the app so the two cannot disagree about where
/// credentials live.
pub fn set_remote_credentials(
    url: &str,
    username: &str,
    password: &str,
) -> Result<(), SignInError> {
    use crate::remote::client::{koan_sign_in, offers_unsigned};
    let url = url.trim_end_matches('/');
    // Asked first, without credentials: the password goes as `p=enc:` only to
    // a server that will trade it for a key.
    if offers_unsigned(url, crate::remote::profile::SIGN_IN).unwrap_or(false) {
        let device = crate::remote::link::LinkIdentity::this_device(None).name;
        match koan_sign_in(url, username, password, &device) {
            Ok(joined) => return adopt_api_key(url, &joined.username, &joined.api_key),
            // Error 50: what was typed is a credential of its own, an app
            // password or the shared secret, which the server will not trade
            // for a key. It is kept as a password is anywhere else.
            Err(SubsonicError::Api { code: 50, .. }) => {}
            Err(e) => return Err(rejected(e)),
        }
    }
    SubsonicClient::new(url, username, password)
        .ping()
        .map_err(rejected)?;

    remember_remote(url, username, Credential::Password(password.to_string()))
}

/// Sign in with an API key made elsewhere — the server's web UI, or another
/// device — for a device where typing a password is the harder thing. Checked
/// against the server before anything is written, as a password is.
pub fn set_remote_api_key(url: &str, username: &str, api_key: &str) -> Result<(), SignInError> {
    let url = url.trim_end_matches('/');
    let credential = Credential::ApiKey(api_key.to_string());
    SubsonicClient::from_auth(SubsonicAuth::with(url, username, credential.clone())).ping()?;
    remember_remote(url, username, credential)
}

/// Change the signed-in account's password on its koan server, keeping this
/// device signed in. The change revokes every key the account had, so the
/// server answers with a new one for this device, which is kept as a sign-in's
/// is.
pub fn change_own_password(current: &str, password: &str) -> Result<(), SignInError> {
    let cfg = Config::load()?;
    let client = subsonic_client(&cfg).ok_or(SignInError::Rejected(SubsonicError::BadResponse))?;
    let device = crate::remote::link::LinkIdentity::this_device(None).name;
    let joined = client.koan_change_own_password(current, password, &device)?;
    adopt_api_key(&cfg.remote.url, &joined.username, &joined.api_key)
}

/// A server's refusal, with error 41 told apart: see `SignInError::NeedsKey`.
fn rejected(e: SubsonicError) -> SignInError {
    match e {
        SubsonicError::Api { code: 41, .. } => SignInError::NeedsKey,
        e => SignInError::Rejected(e),
    }
}

/// Join a server with an invite. A token is traded for an API key named after
/// this device; an address with the account in it carries the password, which
/// signs in as `set_remote_credentials` does.
pub fn join_with_invite(invite: &crate::invite::Invite) -> Result<(), SignInError> {
    let url = invite.server.trim_end_matches('/');
    match (&invite.token, &invite.password) {
        (Some(token), _) => {
            let device = crate::remote::link::LinkIdentity::this_device(None).name;
            let joined = crate::remote::client::redeem_invite(url, token, &device)?;
            adopt_api_key(url, &joined.username, &joined.api_key)
        }
        (None, Some(password)) => set_remote_credentials(url, &invite.username, password),
        (None, None) => Err(SignInError::Rejected(SubsonicError::BadResponse)),
    }
}

/// Sign in with an API key the server just made for this device: by an
/// invite, or by pairing. The key this device held on the same account is
/// revoked, since left valid it would sit in the key list unused; so is the new
/// one if it cannot be stored.
pub(crate) fn adopt_api_key(url: &str, username: &str, api_key: &str) -> Result<(), SignInError> {
    let url = url.trim_end_matches('/');
    let replaced = Config::load()
        .ok()
        .filter(|c| c.remote.url.trim_end_matches('/') == url)
        .filter(|c| c.remote.username == username)
        .map(|c| c.remote.api_key)
        .filter(|k| !k.is_empty() && k != api_key);
    // Revoked best-effort: a key signs in to give itself up.
    let revoke = |key: &str| {
        let credential = Credential::ApiKey(key.to_string());
        let client = SubsonicClient::from_auth(SubsonicAuth::with(url, username, credential));
        if let Err(e) = client.koan_revoke_own_key() {
            log::warn!("could not revoke an unused API key: {e}");
        }
    };
    if let Err(e) = remember_remote(url, username, Credential::ApiKey(api_key.to_string())) {
        revoke(api_key);
        return Err(e);
    }
    if let Some(old) = replaced {
        revoke(&old);
    }
    Ok(())
}

/// Store a credential already checked against the server, replacing whichever
/// kind was there.
fn remember_remote(url: &str, username: &str, credential: Credential) -> Result<(), SignInError> {
    Config::persist(|cfg| {
        cfg.remote.enabled = true;
        cfg.remote.url = url.to_string();
        cfg.remote.username = username.to_string();
        (cfg.remote.password, cfg.remote.api_key) = match &credential {
            Credential::Password(p) => (p.clone(), String::new()),
            Credential::ApiKey(k) => (String::new(), k.clone()),
        };
        // A new keypair with each sign-in, registered against the new API
        // key; a password has no key row to register it on.
        cfg.remote.device_key = match &credential {
            Credential::ApiKey(_) => crate::remote::proof::new_device_key().unwrap_or_default(),
            Credential::Password(_) => String::new(),
        };
    })?;
    // Whatever account was here before, its devices are not this one's, and
    // the link it had open closes, to open again as this one.
    crate::remote::proof::forget();
    crate::remote::link::relink();
    // This device's announcement names the server it is signed in to.
    crate::remote::nearby::readvertise();
    Ok(())
}

/// Shared secret for koan's own Subsonic API.
///
/// Deliberately not the same secret as `remote_credential` — see `SubsonicConfig`.
pub fn get_subsonic_password(cfg: &Config) -> Option<String> {
    (!cfg.subsonic.password.is_empty()).then(|| cfg.subsonic.password.clone())
}

/// Upstream Subsonic credentials from the merged config, returning `None` if
/// remote is disabled or has no URL configured.
///
/// Prefer this over `subsonic_client` when only a signed URL is needed:
/// building a client constructs blocking `reqwest` clients, which panics from
/// inside a tokio runtime.
pub fn subsonic_auth(cfg: &Config) -> Option<SubsonicAuth> {
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        return None;
    }
    Some(SubsonicAuth::with(
        &cfg.remote.url,
        &cfg.remote.username,
        remote_credential(cfg)?,
    ))
}

/// One `SubsonicClient` per set of credentials, shared process-wide.
///
/// Constructing one builds two blocking `reqwest` clients, each carrying its
/// own runtime on its own thread, and each starting with a cold connection
/// pool — so a client per call means a fresh TLS handshake for every cover art
/// request.
///
/// Keyed on the credentials, so logging in as someone else replaces the client
/// rather than serving the old one. Never call from async code: building the
/// inner clients panics inside a tokio runtime.
pub fn subsonic_client(cfg: &Config) -> Option<Arc<SubsonicClient>> {
    let auth = subsonic_auth(cfg)?;

    let mut slot = SUBSONIC_CLIENT.lock();
    if let Some((cached, client)) = slot.as_ref()
        && *cached == auth
    {
        return Some(client.clone());
    }

    let client = Arc::new(SubsonicClient::from_auth(auth.clone()));
    *slot = Some((auth, client.clone()));
    Some(client)
}

type CachedClient = Option<(SubsonicAuth, Arc<SubsonicClient>)>;

static SUBSONIC_CLIENT: std::sync::LazyLock<parking_lot::Mutex<CachedClient>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(None));

/// Why there is no remote client, in words worth showing someone.
///
/// Every caller of `subsonic_client` gets `None` for three different reasons,
/// and reporting one for all of them makes "koan has no password", which sends
/// you to sign in, look like a server that is merely down.
pub fn remote_unavailable(cfg: &Config) -> String {
    if !cfg.remote.enabled {
        return "no remote server is configured".into();
    }
    if cfg.remote.url.is_empty() {
        return "the remote server has no address".into();
    }
    if remote_credential(cfg).is_none() {
        return "no password or API key is stored for the remote server".into();
    }
    // A credential resolved, so the client should have built. Nothing else
    // returns `None`, but saying so beats claiming a cause that is wrong.
    "the remote server could not be reached".into()
}

/// What every front end says when the server refused the stored credential.
pub const SIGN_IN_REFUSED: &str = "the remote server refused the stored sign-in; sign in again";

/// Why the configured server cannot be used, if it cannot: no credential, or
/// one the server has refused since (a revoked API key, a changed password).
/// `None` when no server is configured, or the one that is works as far as
/// anything has heard.
pub fn remote_problem(cfg: &Config) -> Option<String> {
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        return None;
    }
    match subsonic_auth(cfg) {
        None => Some(remote_unavailable(cfg)),
        Some(auth) if crate::remote::refusal::refused(&auth) => Some(SIGN_IN_REFUSED.into()),
        Some(_) => None,
    }
}

/// Whether the server refused the configured credential when it was last used.
pub fn sign_in_refused(cfg: &Config) -> bool {
    subsonic_auth(cfg).is_some_and(|auth| crate::remote::refusal::refused(&auth))
}

#[cfg(test)]
mod client_cache_tests {
    use super::*;

    #[test]
    fn one_subsonic_client_is_shared_per_credentials() {
        crate::config::isolate_config_for_tests();
        let mut cfg = Config::default();
        cfg.remote.enabled = true;
        cfg.remote.url = "https://shared-client.invalid".into();
        cfg.remote.username = "koan".into();
        cfg.remote.password = "first".into();

        let first = subsonic_client(&cfg).expect("a configured remote yields a client");
        let again = subsonic_client(&cfg).expect("a configured remote yields a client");
        assert!(
            Arc::ptr_eq(&first, &again),
            "rebuilding drops the connection pool and re-handshakes TLS per request"
        );

        cfg.remote.password = "second".into();
        let relogged = subsonic_client(&cfg).expect("a configured remote yields a client");
        assert!(
            !Arc::ptr_eq(&first, &relogged),
            "new credentials must not keep serving the client signed with the old ones"
        );
    }
}

#[cfg(test)]
mod sign_in_tests {
    use super::*;

    /// A koan server as `set_remote_credentials` meets it over plain HTTP:
    /// `mate`'s account password is traded for a key, while `mate`'s app
    /// password and `testuser`'s shared secret are refused that trade with
    /// error 50 and accepted as tokens.
    fn serve() -> String {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let target = request.split_whitespace().nth(1).unwrap_or("");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let param = |name: &str| {
                    query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(k, _)| *k == name)
                        .map(|(_, v)| v.replace("%3A", ":"))
                        .unwrap_or_default()
                };
                let secret = match param("u").as_str() {
                    "mate" => "app-secret",
                    "testuser" => "shared-secret",
                    _ => "",
                };
                let ok = r#"{"subsonic-response":{"status":"ok"}}"#.to_owned();
                let refused = |code: i32| {
                    format!(
                        r#"{{"subsonic-response":{{"status":"failed","error":{{"code":{code},"message":"refused"}}}}}}"#
                    )
                };
                let body = match path.rsplit('/').next().unwrap() {
                    "getOpenSubsonicExtensions" => r#"{"subsonic-response":{"status":"ok","openSubsonicExtensions":[{"name":"koanSignIn","versions":[1]}]}}"#.to_owned(),
                    "koanSignIn" => {
                        let hex = param("p").trim_start_matches("enc:").to_owned();
                        let typed: String = (0..hex.len())
                            .step_by(2)
                            .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                            .map(char::from)
                            .collect();
                        match (param("u").as_str(), typed.as_str()) {
                            ("mate", "hunter22") => r#"{"subsonic-response":{"status":"ok","join":{"username":"mate","apiKey":"minted"}}}"#.to_owned(),
                            (_, typed) if typed == secret => refused(50),
                            _ => refused(40),
                        }
                    }
                    "ping" if param("apiKey") == "minted" => ok,
                    "ping" => {
                        let expected =
                            format!("{:x}", md5::compute(format!("{secret}{}", param("s"))));
                        if !secret.is_empty() && param("t") == expected {
                            ok
                        } else {
                            refused(40)
                        }
                    }
                    _ => ok,
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn a_password_ends_in_a_key_and_other_credentials_are_kept_as_typed() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::config::set_config_dir(dir.path());
        let url = serve();
        let kept = || {
            let remote = Config::load().unwrap().remote;
            (remote.username, remote.password, remote.api_key)
        };

        set_remote_credentials(&url, "mate", "hunter22").unwrap();
        assert_eq!(kept(), ("mate".into(), String::new(), "minted".into()));

        set_remote_credentials(&url, "mate", "app-secret").unwrap();
        assert_eq!(
            kept(),
            ("mate".into(), "app-secret".into(), String::new()),
            "an app password is refused a key and kept"
        );

        set_remote_credentials(&url, "testuser", "shared-secret").unwrap();
        assert_eq!(
            kept(),
            ("testuser".into(), "shared-secret".into(), String::new()),
            "the shared secret is refused a key and kept"
        );

        assert!(matches!(
            set_remote_credentials(&url, "mate", "wrong"),
            Err(SignInError::Rejected(SubsonicError::Api { code: 40, .. }))
        ));
        assert_eq!(
            kept().1,
            "shared-secret",
            "a refused sign-in writes nothing"
        );
    }
}

#[cfg(test)]
mod refusal_tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// A koan server holding API keys that can be revoked. `koanSignIn` with
    /// `mate`'s password mints `second`.
    fn serve(keys: Arc<Mutex<HashSet<String>>>) -> String {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let target = request.split_whitespace().nth(1).unwrap_or("");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let param = |name: &str| {
                    query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(k, _)| *k == name)
                        .map(|(_, v)| v.to_owned())
                        .unwrap_or_default()
                };
                let endpoint = path.rsplit('/').next().unwrap();
                let body = if endpoint == "koanSignIn" {
                    keys.lock().unwrap().insert("second".into());
                    r#"{"subsonic-response":{"status":"ok","join":{"username":"mate","apiKey":"second"}}}"#.to_owned()
                } else if endpoint == "getOpenSubsonicExtensions" {
                    r#"{"subsonic-response":{"status":"ok","openSubsonicExtensions":[{"name":"koanSignIn","versions":[1]}]}}"#.to_owned()
                } else if !keys.lock().unwrap().contains(&param("apiKey")) {
                    r#"{"subsonic-response":{"status":"failed","error":{"code":44,"message":"invalid API key"}}}"#.to_owned()
                } else {
                    r#"{"subsonic-response":{"status":"ok","indexes":{"lastModified":1}}}"#
                        .to_owned()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn a_revoked_key_is_reported_until_signing_in_again() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::config::set_config_dir(dir.path());
        let keys = Arc::new(Mutex::new(HashSet::from(["first".to_owned()])));
        let url = serve(keys.clone());
        Config::persist(|c| {
            c.remote.enabled = true;
            c.remote.url = url.clone();
            c.remote.username = "mate".into();
            c.remote.api_key = "first".into();
        })
        .unwrap();

        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let sync = || {
            let cfg = Config::load().unwrap();
            let client = SubsonicClient::from_auth(subsonic_auth(&cfg).unwrap());
            let _ = sync_remote(&db, &client, Walk::IfChanged, &url, "mate", &|_| {});
        };

        sync();
        assert_eq!(remote_problem(&Config::load().unwrap()), None);

        keys.lock().unwrap().remove("first");
        sync();
        let cfg = Config::load().unwrap();
        assert_eq!(remote_problem(&cfg).as_deref(), Some(SIGN_IN_REFUSED));
        assert!(sign_in_refused(&cfg));

        set_remote_credentials(&url, "mate", "hunter22").unwrap();
        let cfg = Config::load().unwrap();
        assert_eq!(cfg.remote.api_key, "second");
        assert_eq!(remote_problem(&cfg), None, "a new credential starts clean");
        sync();
        assert_eq!(remote_problem(&Config::load().unwrap()), None);
    }
}

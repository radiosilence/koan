//! Sharing this device's library: koan-server, on the engine's own player.
//!
//! While it runs, the account settings that administer a server (people,
//! invites, devices, pairing) administer this one, over loopback with the
//! server's host key, through the same Subsonic calls a remote server gets.

use std::sync::Arc;

use koan_core::db::pool::Pool;
use koan_core::remote::client::{Credential, SubsonicAuth, SubsonicClient};
use koan_server::graphql::{ApiServer, ApiServerOpts, Profile};
use parking_lot::RwLock;

use crate::offload;
use crate::{KoanEngine, KoanError};

/// Where the library is being served, while it is.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct Sharing {
    pub port: u16,
    /// Where a phone on this network reaches it, e.g.
    /// `http://192.168.1.20:4000`. `None` with no network to be reached on.
    pub lan_url: Option<String>,
}

/// Tailscale on this device, for Settings.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct TailscaleStatus {
    /// The device's name on the tailnet, `mac.tailnet.ts.net`.
    pub name: String,
    /// The tailnet lets this device use Funnel.
    pub funnel_allowed: bool,
    /// Where Funnel publishes the library, while it does.
    pub funnel_url: Option<String>,
}

fn funnel_error(e: koan_core::tailscale::FunnelError) -> KoanError {
    KoanError::Refused {
        message: e.to_string(),
    }
}

/// Publish the server on its new `port` after a start, taking the Funnel off
/// the port it had `before`, and keep the public address on Funnel's: the
/// machine's tailnet name, or Funnel's port, may have changed meanwhile.
fn republish(before: Option<u16>, port: u16) {
    if let Some(before) = before.filter(|&b| b != port)
        && let Err(e) = koan_core::tailscale::funnel_off(before)
    {
        log::warn!("share: Funnel on the old port left up: {e}");
    }
    match koan_core::tailscale::funnel(port) {
        Ok(url) => {
            let changed = koan_core::config::Config::cached().server.url.as_deref() != Some(&url);
            if changed
                && let Err(e) = koan_core::config::Config::persist(|cfg| cfg.server.url = Some(url))
            {
                log::warn!("share: Funnel's address not saved: {e}");
            }
        }
        Err(e) => log::warn!("share: Funnel not published: {e}"),
    }
}

/// The running server's port and its client. Set once it has started and
/// cleared as it begins to stop, so a read never waits on a stop's drain.
struct Serving {
    port: u16,
    /// The server administered as its first admin, made once per run.
    client: Arc<SubsonicClient>,
    /// What the server said of itself when it started: the extensions
    /// Settings offers its sections by.
    profile: Option<koan_core::remote::profile::ServerProfile>,
}

static SERVING: RwLock<Option<Serving>> = RwLock::new(None);

fn sharing(port: u16) -> Sharing {
    Sharing {
        port,
        lan_url: koan_core::remote::devices::lan_address().map(|ip| format!("http://{ip}:{port}")),
    }
}

fn client(port: u16, key: &str) -> Arc<SubsonicClient> {
    Arc::new(SubsonicClient::from_auth(SubsonicAuth::with(
        &format!("http://127.0.0.1:{port}"),
        "",
        Credential::ApiKey(key.to_owned()),
    )))
}

/// The running server, administered as its first admin. `None` while not
/// sharing.
pub(crate) fn loopback_client() -> Option<Arc<SubsonicClient>> {
    Some(SERVING.read().as_ref()?.client.clone())
}

/// The running server's own profile, which stands in for a remote's while
/// sharing. Kept here, apart from `profile`'s, which belongs to the server
/// this device signs in to.
pub(crate) fn own_profile() -> Option<koan_core::remote::profile::ServerProfile> {
    SERVING.read().as_ref()?.profile.clone()
}

/// Signing in to a server is refused while this device serves its own
/// library: its account settings would administer one while it played the
/// other's.
pub(crate) fn refuse_while_sharing() -> Result<(), KoanError> {
    match SERVING.read().is_some() {
        true => Err(KoanError::Refused {
            message: "this device is sharing its library; stop sharing to sign in to a server"
                .into(),
        }),
        false => Ok(()),
    }
}

/// The address invites send phones to while sharing: `server.url` where
/// Funnel or a tunnel publishes this device; else its tailnet name, where
/// Tailscale runs and invites go there; else its address on the network.
pub(crate) fn invite_base() -> Option<String> {
    let port = SERVING.read().as_ref()?.port;
    let cfg = koan_core::config::Config::cached();
    cfg.server_url()
        .or_else(|| {
            cfg.share
                .tailnet_invites
                .then(koan_core::tailscale::status)
                .flatten()
                .map(|t| format!("http://{}:{port}", t.name))
        })
        .or_else(|| sharing(port).lan_url)
}

#[uniffi::export]
impl KoanEngine {
    /// Serve the library on every interface, on `port` (0 for any free one).
    /// `names` are what else the server answers to as a host, such as this
    /// Mac's Bonjour name. A server already running is stopped first, so a
    /// start that fails, on a port taken, leaves nothing serving.
    pub async fn start_server(
        self: Arc<Self>,
        port: u16,
        names: Vec<String>,
    ) -> Result<Sharing, KoanError> {
        offload::offload(move || {
            if koan_core::helpers::remote_credential(&koan_core::config::Config::cached()).is_some()
            {
                return Err(KoanError::Refused {
                    message:
                        "this device is signed in to a server; sign out to share its own library"
                            .into(),
                });
            }
            // Held throughout, so a start and a stop never interleave.
            let mut server = self.server.lock();
            let before = SERVING.write().take().map(|s| s.port);
            if let Some(running) = server.take() {
                running.stop();
            }
            // Phones on the tailnet reach this device by its name there.
            let tailscale = koan_core::tailscale::status();
            let mut names = names;
            names.extend(tailscale.as_ref().map(|t| t.name.clone()));
            let started = ApiServer::start(ApiServerOpts {
                state: self.state.clone(),
                cmd_tx: self.tx.clone(),
                pool: Arc::new(Pool::new(koan_core::config::db_path())),
                port: Some(port),
                bind: Some(std::net::Ipv4Addr::UNSPECIFIED.into()),
                subsonic_port: None,
                playground: false,
                viz: Some(self.viz.clone()),
                profile: Profile::Embedded,
                hosts: names,
            })
            .map_err(|message| KoanError::Refused { message })?;
            let port = started.addr().port();
            let key = started
                .host_key()
                .expect("an embedded server has a host key");
            let client = client(port, key);
            *SERVING.write() = Some(Serving {
                port,
                profile: client.profile().ok(),
                client,
            });
            *server = Some(started);
            koan_core::signal::engine_changed().bump();
            if koan_core::config::Config::cached().share.funnel {
                republish(before, port);
            }
            Ok(sharing(port))
        })
        .await
    }

    /// Stop serving, closing every open connection. Nothing if no server
    /// runs. Can take as long as the server's drain; nothing waits on it at
    /// quit, when the process closing does the same.
    pub async fn stop_server(self: Arc<Self>) {
        offload::offload(move || {
            let mut server = self.server.lock();
            let port = SERVING.write().take().map(|s| s.port);
            koan_core::signal::engine_changed().bump();
            // Taken down with the server, which it would otherwise answer
            // for with errors; `share.funnel` stays, for the next start.
            if koan_core::config::Config::cached().share.funnel
                && let Some(port) = port
                && let Err(e) = koan_core::tailscale::funnel_off(port)
            {
                log::warn!("share: Funnel left up: {e}");
            }
            if let Some(running) = server.take() {
                running.stop();
            }
        })
        .await
    }

    /// Where the library is served, while it is, with the network address
    /// worked out afresh: it moves when the Mac changes network.
    pub async fn server_sharing(self: Arc<Self>) -> Option<Sharing> {
        offload::offload(move || {
            let port = SERVING.read().as_ref()?.port;
            Some(sharing(port))
        })
        .await
    }

    /// Tailscale on this device, while it is running and signed in.
    pub async fn tailscale(self: Arc<Self>) -> Option<TailscaleStatus> {
        offload::offload(move || {
            let tailscale = koan_core::tailscale::status()?;
            let port = SERVING.read().as_ref().map(|s| s.port);
            let funnel_url = port
                .filter(|_| koan_core::config::Config::cached().share.funnel)
                .and_then(koan_core::tailscale::funnel_port)
                .map(|on| tailscale.url_on(on));
            Some(TailscaleStatus {
                name: tailscale.name,
                funnel_allowed: tailscale.funnel_allowed,
                funnel_url,
            })
        })
        .await
    }

    /// Publish the library on the internet through Tailscale Funnel, now and
    /// whenever it is served, and send invites to its address there.
    /// Returns the address.
    pub async fn enable_funnel(self: Arc<Self>) -> Result<String, KoanError> {
        offload::offload(move || {
            let port =
                SERVING
                    .read()
                    .as_ref()
                    .map(|s| s.port)
                    .ok_or_else(|| KoanError::Refused {
                        message: "start sharing first".into(),
                    })?;
            let url = koan_core::tailscale::funnel(port).map_err(funnel_error)?;
            let address = url.clone();
            koan_core::config::Config::persist(move |cfg| {
                cfg.share.funnel = true;
                cfg.server.url = Some(address);
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })?;
            Ok(url)
        })
        .await
    }

    /// Stop publishing through Funnel. The public address goes with it, where
    /// it was Funnel's.
    pub async fn disable_funnel(self: Arc<Self>) -> Result<(), KoanError> {
        offload::offload(move || {
            let tailscale = koan_core::tailscale::status();
            if let Some(port) = SERVING.read().as_ref().map(|s| s.port) {
                koan_core::tailscale::funnel_off(port).map_err(funnel_error)?;
            }
            koan_core::config::Config::persist(move |cfg| {
                cfg.share.funnel = false;
                // Any of Funnel's addresses for this machine is Funnel's.
                let funnels = |url: &str| {
                    tailscale
                        .as_ref()
                        .is_some_and(|t| url.starts_with(&format!("https://{}", t.name)))
                };
                if cfg.server.url.as_deref().is_some_and(funnels) {
                    cfg.server.url = None;
                }
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
        })
        .await
    }

    /// The account the library is shared under, its first admin: the one
    /// this device administers the server as. `None` until one is made, and
    /// sharing starts only once there is one, since nobody could sign in.
    pub async fn server_admin(self: Arc<Self>) -> Result<Option<String>, KoanError> {
        offload::offload(move || {
            use koan_core::db::queries::auth;
            let db = self.db()?;
            let Some(id) = auth::first_admin(&db.conn).map_err(crate::db_err)? else {
                return Ok(None);
            };
            Ok(auth::get_user_by_id(&db.conn, id)
                .map_err(crate::db_err)?
                .map(|u| u.username))
        })
        .await
    }

    /// Make the first account, an admin, in this library's database: the one
    /// sharing begins with, and the one this device administers it as.
    /// Refused once any account exists, with the server's own rules for names
    /// and passwords.
    pub async fn create_first_server_account(
        self: Arc<Self>,
        username: String,
        password: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            koan_core::invite::create_first_admin(&db.conn, &username, &password)
                .map(|_| ())
                .map_err(|e| KoanError::BadArgument {
                    message: e.to_string(),
                })
        })
        .await
    }
}

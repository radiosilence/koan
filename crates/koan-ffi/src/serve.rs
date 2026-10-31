//! Sharing this device's library: koan-server, on the engine's own player.
//!
//! While it runs, the account settings that administer a server (people,
//! invites, devices, pairing) administer this one, over loopback with the
//! server's host key, through the same Subsonic calls a remote server gets.
//!
//! Where invites send phones is `share.invite_to`, and the engine is the only
//! writer of it and of `server.url` (Funnel's address, or a custom one), so
//! Settings never saves an older copy over a change made here.

use std::sync::Arc;

use koan_core::config::{Config, InviteTo as Target};
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
}

/// Where invites send phones.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteTo {
    /// The device's address on the local network.
    Network,
    /// Its Tailscale name, for phones with Tailscale, anywhere.
    Tailnet,
    /// Its address on the internet through Tailscale Funnel, published
    /// whenever it serves.
    Funnel,
    /// A tunnel's address, typed in Settings.
    Custom,
}

impl From<Target> for InviteTo {
    fn from(t: Target) -> Self {
        match t {
            Target::Network => Self::Network,
            Target::Tailnet => Self::Tailnet,
            Target::Funnel => Self::Funnel,
            Target::Custom => Self::Custom,
        }
    }
}

impl From<InviteTo> for Target {
    fn from(t: InviteTo) -> Self {
        match t {
            InviteTo::Network => Self::Network,
            InviteTo::Tailnet => Self::Tailnet,
            InviteTo::Funnel => Self::Funnel,
            InviteTo::Custom => Self::Custom,
        }
    }
}

/// Where invites go and the addresses Settings shows, as the engine works
/// them out, read together so the page never mixes two moments.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct ShareStatus {
    pub to: InviteTo,
    /// Tailscale on this device, while it is running and signed in.
    pub tailscale: Option<TailscaleStatus>,
    /// Where invites send phones now. The addresses are `None` while not
    /// serving.
    pub current: Option<String>,
    /// Where they go with no custom address typed: the tailnet's, else the
    /// network's.
    pub fallback: Option<String>,
    /// Where Funnel publishes the library, while it does.
    pub funnel: Option<String>,
}

fn funnel_error(e: koan_core::tailscale::FunnelError) -> KoanError {
    KoanError::Refused {
        message: e.to_string(),
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

fn serving_port() -> Option<u16> {
    SERVING.read().as_ref().map(|s| s.port)
}

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

/// Where invites go, and the addresses for `port` while serving.
fn status(port: Option<u16>) -> ShareStatus {
    let cfg = Config::cached();
    let tailscale = koan_core::tailscale::status();
    let Some(port) = port else {
        return ShareStatus {
            to: cfg.share.invite_to.into(),
            tailscale: tailscale.map(Into::into),
            current: None,
            fallback: None,
            funnel: None,
        };
    };
    let tailnet = tailscale
        .as_ref()
        .map(|t| format!("http://{}:{port}", t.name));
    let fallback = tailnet.clone().or_else(|| sharing(port).lan_url);
    let funnel = (cfg.share.invite_to == Target::Funnel)
        .then(|| {
            let t = tailscale.as_ref()?;
            koan_core::tailscale::funnel_port(port).map(|on| t.url_on(on))
        })
        .flatten();
    let current = match cfg.share.invite_to {
        Target::Network => None,
        Target::Tailnet => tailnet,
        Target::Funnel => funnel.clone().or_else(|| fallback.clone()),
        Target::Custom => cfg.server_url().or_else(|| fallback.clone()),
    }
    .or_else(|| sharing(port).lan_url);
    ShareStatus {
        to: cfg.share.invite_to.into(),
        tailscale: tailscale.map(Into::into),
        current,
        fallback,
        funnel,
    }
}

impl From<koan_core::tailscale::Tailscale> for TailscaleStatus {
    fn from(t: koan_core::tailscale::Tailscale) -> Self {
        Self {
            name: t.name,
            funnel_allowed: t.funnel_allowed,
        }
    }
}

/// The address invites send phones to while sharing.
pub(crate) fn invite_base() -> Option<String> {
    status(Some(serving_port()?)).current
}

/// The host in `url`, `https://host:port/path` -> `host`.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    (!host.is_empty()).then(|| host.to_owned())
}

/// Publish on Funnel for `port` and keep `server.url` on its address, which
/// share links and the server's own idea of where it is reached are built
/// on. The address moves with the machine's tailnet name and Funnel's port.
fn publish(port: u16) -> Result<(), KoanError> {
    let url = koan_core::tailscale::funnel(port).map_err(funnel_error)?;
    if Config::cached().server.url.as_deref() != Some(&url) {
        Config::persist(|cfg| cfg.server.url = Some(url)).map_err(|e| KoanError::BadArgument {
            message: e.to_string(),
        })?;
    }
    Ok(())
}

/// Take down whatever Funnel publishes for `port`, when nothing should.
fn unpublish(port: u16) {
    if let Err(e) = koan_core::tailscale::funnel_off(port) {
        log::debug!("share: Funnel not taken down: {e}");
    }
}

/// Give `username` `password` in this library, as its host, and end its
/// devices' links, as the server does for a password it changes.
pub(crate) fn reset_password(
    conn: &rusqlite::Connection,
    username: &str,
    password: &str,
) -> Result<(), KoanError> {
    koan_core::invite::set_password(conn, username, Some(password)).map_err(|e| {
        KoanError::BadArgument {
            message: e.to_string(),
        }
    })?;
    koan_server::clients::registry().disconnect(username);
    Ok(())
}

/// The account the library is shared under: its first admin.
fn admin(conn: &rusqlite::Connection) -> Result<Option<String>, KoanError> {
    use koan_core::db::queries::auth;
    let Some(id) = auth::first_admin(conn).map_err(crate::db_err)? else {
        return Ok(None);
    };
    Ok(auth::get_user_by_id(conn, id)
        .map_err(crate::db_err)?
        .map(|u| u.username))
}

/// Whether `username` is the account this device shares its library under,
/// while it shares: a password change for it goes through `reset_password`,
/// since the server would take the host key's caller for the account itself.
pub(crate) fn is_shared_admin(conn: &rusqlite::Connection, username: &str) -> bool {
    SERVING.read().is_some() && admin(conn).ok().flatten().as_deref() == Some(username)
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
            if koan_core::helpers::remote_credential(&Config::cached()).is_some() {
                return Err(KoanError::Refused {
                    message:
                        "this device is signed in to a server; sign out to share its own library"
                            .into(),
                });
            }
            // Held throughout, so a start and a stop never interleave.
            let mut server = self.server.lock();
            if let Some(before) = SERVING.write().take().map(|s| s.port) {
                unpublish(before);
            }
            if let Some(running) = server.take() {
                running.stop();
            }
            // Phones on the tailnet reach this device by its name there, and
            // a tunnel's by the custom address.
            let mut names = names;
            names.extend(koan_core::tailscale::status().map(|t| t.name));
            names.extend(Config::cached().server_url().as_deref().and_then(host_of));
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
            // Settings offers its sections by the connection slice, which is
            // published again when this moves.
            koan_core::remote::devices::touch();
            // A Funnel does not survive Tailscale restarting, and the port
            // may have moved: published again wherever it was chosen.
            if Config::cached().share.invite_to == Target::Funnel
                && let Err(e) = publish(port)
            {
                log::warn!("share: Funnel not published: {e}");
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
            koan_core::remote::devices::touch();
            // Taken down with the server, which it would otherwise answer for
            // with errors, and put back on the next start.
            if let Some(port) = port {
                unpublish(port);
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
        offload::offload(move || Some(sharing(serving_port()?))).await
    }

    /// Where invites go, Tailscale, and the addresses Settings shows.
    pub async fn share_status(self: Arc<Self>) -> ShareStatus {
        offload::offload(move || status(serving_port())).await
    }

    /// Where invites send phones (`share.invite_to`). `Funnel` publishes the
    /// library on the internet through Tailscale Funnel, now and whenever it
    /// is served; any other choice stops publishing. `Custom` sends them to
    /// `address` (`server.url`), kept as it was when `None`; any other choice
    /// clears it, since the server tells phones it is the address it is known
    /// by.
    pub async fn set_invite_to(
        self: Arc<Self>,
        to: InviteTo,
        address: Option<String>,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let address = address
                .map(|a| a.trim().trim_end_matches('/').to_owned())
                .map(|a| {
                    match a.is_empty() || a.starts_with("https://") || a.starts_with("http://") {
                        true => Ok(a),
                        false => Err(KoanError::BadArgument {
                            message: "an address starts with https:// or http://".into(),
                        }),
                    }
                })
                .transpose()?;
            let port = serving_port();
            let was_funnel = Config::cached().share.invite_to == Target::Funnel;
            if to == InviteTo::Funnel {
                let port = port.ok_or_else(|| KoanError::Refused {
                    message: "start sharing first".into(),
                })?;
                publish(port)?;
            } else {
                // Not serving, as after a failed start, the Funnel to take
                // down is the one for the port it was told. Leaving Funnel,
                // a failure is the library left on the internet, so it is
                // said; otherwise there was most likely nothing to take down.
                let port = port.unwrap_or(Config::cached().share.port);
                if was_funnel {
                    koan_core::tailscale::funnel_off(port).map_err(funnel_error)?;
                } else {
                    unpublish(port);
                }
            }
            Config::persist(move |cfg| {
                cfg.share.invite_to = to.into();
                match (to, address) {
                    (InviteTo::Custom, Some(a)) => cfg.server.url = (!a.is_empty()).then_some(a),
                    // Funnel's address is not a custom one to keep.
                    (InviteTo::Custom, None) if was_funnel => cfg.server.url = None,
                    // Funnel's was written by `publish`; a custom one kept.
                    (InviteTo::Custom | InviteTo::Funnel, None) => {}
                    _ => cfg.server.url = None,
                }
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
        })
        .await
    }

    /// Give the account the library is shared under a new password. This
    /// device is the server's host, so the old one is not asked for. Signs
    /// the account's other devices out.
    pub async fn reset_server_admin_password(
        self: Arc<Self>,
        password: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let username = admin(&db.conn)?.ok_or_else(|| KoanError::NotFound {
                message: "this library has no account yet".into(),
            })?;
            reset_password(&db.conn, &username, &password)
        })
        .await
    }

    /// The account the library is shared under, its first admin: the one
    /// this device administers the server as. `None` until one is made, and
    /// sharing starts only once there is one, since nobody could sign in.
    pub async fn server_admin(self: Arc<Self>) -> Result<Option<String>, KoanError> {
        offload::offload(move || admin(&self.db()?.conn)).await
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

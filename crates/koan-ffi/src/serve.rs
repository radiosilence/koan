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

/// The running server's port and host key. Set once it has started and
/// cleared as it begins to stop, so a read never waits on a stop's drain.
struct Serving {
    port: u16,
    key: String,
    /// What the server said of itself when it started: the extensions
    /// Settings offers its sections by.
    profile: Option<koan_core::remote::profile::ServerProfile>,
}

static SERVING: RwLock<Option<Serving>> = RwLock::new(None);

/// The address of the interface the default route leaves by: the one a phone
/// on the same network reaches. A UDP socket connects without sending.
fn lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("1.1.1.1", 80)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

fn sharing(port: u16) -> Sharing {
    Sharing {
        port,
        lan_url: lan_ip().map(|ip| format!("http://{ip}:{port}")),
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
    let serving = SERVING.read();
    let serving = serving.as_ref()?;
    Some(client(serving.port, &serving.key))
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

/// The address invites send phones to while sharing: `server.url` where a
/// tunnel publishes this device, otherwise its address on the network.
pub(crate) fn invite_base() -> Option<String> {
    let port = SERVING.read().as_ref()?.port;
    koan_core::config::Config::cached()
        .server_url()
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
            *SERVING.write() = None;
            if let Some(running) = server.take() {
                running.stop();
            }
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
                .expect("an embedded server has a host key")
                .to_owned();
            *SERVING.write() = Some(Serving {
                port,
                profile: client(port, &key).profile().ok(),
                key,
            });
            *server = Some(started);
            koan_core::signal::engine_changed().bump();
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
            *SERVING.write() = None;
            koan_core::signal::engine_changed().bump();
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

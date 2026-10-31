//! Sharing this device's library: koan-server, on the engine's own player.

use std::sync::Arc;

use koan_core::db::pool::Pool;
use koan_server::graphql::{ApiServer, ApiServerOpts, Profile};

use crate::offload;
use crate::{KoanEngine, KoanError};

#[uniffi::export]
impl KoanEngine {
    /// Serve the library on `port` (0 for any free one), on every interface
    /// unless `bind` names an address. A server already running is stopped
    /// first. Returns the address it is listening on.
    pub async fn start_server(
        self: Arc<Self>,
        port: u16,
        bind: Option<String>,
    ) -> Result<String, KoanError> {
        let bind = bind
            .map(|b| {
                b.parse::<std::net::IpAddr>()
                    .map_err(|_| KoanError::BadArgument {
                        message: format!("{b:?} is not an IP address"),
                    })
            })
            .transpose()?
            .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED.into());
        offload::offload(move || {
            // Held throughout, so a start and a stop never interleave.
            let mut server = self.server.lock();
            if let Some(running) = server.take() {
                running.stop();
            }
            let started = ApiServer::start(
                ApiServerOpts {
                    state: self.state.clone(),
                    cmd_tx: self.tx.clone(),
                    pool: Arc::new(Pool::new(koan_core::config::db_path())),
                    port: Some(port),
                    bind: Some(bind),
                    subsonic_port: None,
                    playground: false,
                    viz: Some(self.viz.clone()),
                    profile: Profile::Embedded,
                },
                Default::default(),
            )
            .map_err(|message| KoanError::Refused { message })?;
            let addr = started.addr().to_string();
            *server = Some(started);
            Ok(addr)
        })
        .await
    }

    /// Stop serving, closing every open connection. Nothing if no server runs.
    pub async fn stop_server(self: Arc<Self>) {
        offload::offload(move || {
            if let Some(running) = self.server.lock().take() {
                running.stop();
            }
        })
        .await
    }

    /// Where the server is listening, while one runs.
    pub async fn server_address(self: Arc<Self>) -> Option<String> {
        offload::offload(move || self.server.lock().as_ref().map(|s| s.addr().to_string())).await
    }
}

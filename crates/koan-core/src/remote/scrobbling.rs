//! The signed-in account's scrobbling, set up through its koan server
//! (`koanScrobbling`). The server holds the ListenBrainz token and sends the
//! plays, so the apps only ask, connect and disconnect. The sending is
//! `crate::scrobbling`'s, on the server.

use crate::config::Config;
use crate::db::queries::scrobbling::LISTENBRAINZ;
use crate::remote::client::{KoanScrobbleService, SubsonicClient, SubsonicError};

#[derive(Debug, thiserror::Error)]
pub enum ScrobblingError {
    #[error("no server is signed in")]
    NotSignedIn,
    #[error(transparent)]
    Remote(#[from] SubsonicError),
}

fn client() -> Result<std::sync::Arc<SubsonicClient>, ScrobblingError> {
    crate::helpers::subsonic_client(&Config::load().unwrap_or_default())
        .ok_or(ScrobblingError::NotSignedIn)
}

fn listenbrainz(services: Vec<KoanScrobbleService>) -> Option<KoanScrobbleService> {
    services.into_iter().find(|s| s.name == LISTENBRAINZ)
}

/// The account's ListenBrainz connection, if it has one.
pub fn status() -> Result<Option<KoanScrobbleService>, ScrobblingError> {
    Ok(listenbrainz(client()?.koan_scrobbling()?.service))
}

/// Connect the account's ListenBrainz with a user token. A token
/// ListenBrainz refuses comes back as the server's message.
pub fn connect(token: &str) -> Result<Option<KoanScrobbleService>, ScrobblingError> {
    Ok(listenbrainz(
        client()?.koan_scrobbling_connect(token)?.service,
    ))
}

pub fn disconnect() -> Result<(), ScrobblingError> {
    client()?.koan_scrobbling_disconnect()?;
    Ok(())
}

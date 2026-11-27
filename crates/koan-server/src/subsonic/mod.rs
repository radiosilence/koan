//! Subsonic-compatible REST API layer.
//!
//! Implements a subset of the Subsonic/OpenSubsonic REST API backed by the
//! local koan database.  Supports both XML (default) and JSON (`f=json`)
//! responses.  Clients sign in with a koan account, or with the dedicated
//! `[subsonic]` secret — see `validate_auth`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRequestParts, Path as UrlPath, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use koan_core::auth::Role;
use koan_core::config::Config;
use koan_core::db::connection::Database;
use koan_core::db::pool::{Handle, Pool};
use koan_core::db::queries;
use koan_core::db::queries::app_passwords::AppPasswordAuth;
use koan_core::remote::client::SubsonicAuth;
use serde::Deserialize;
use tokio::io::AsyncReadExt as _;

use crate::auth::password::{FailureLimiter, PasswordVerifier};

mod auth;
mod bookmarks;
mod browsing;
mod convert;
mod covers;
mod error;
mod ids;
mod interaction;
mod koan;
mod params;
mod playlists;
mod prologue;
mod response;
mod search;
mod streaming;
mod system;
#[cfg(test)]
mod testing;
mod unimplemented;

use auth::*;
use bookmarks::*;
use browsing::*;
use convert::*;
use covers::*;
use error::*;
use ids::*;
use interaction::*;
use koan::*;
use params::*;
use playlists::*;
use prologue::*;
use response::*;
use search::*;
use streaming::*;
use system::*;
use unimplemented::*;

pub(crate) use convert::track_file_path;
pub(crate) use ids::EntityKind;
pub(crate) use interaction::{favourite, favourited};
pub(crate) use response::not_authorized;
pub(crate) use streaming::serve_local_file;

const SUBSONIC_API_VERSION: &str = "1.16.1";
const SUBSONIC_XMLNS: &str = "http://subsonic.org/restapi";
/// The OpenSubsonic `type`: which server this is, as opposed to which protocol.
const SERVER_TYPE: &str = "koan";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where an authentication error sends a client's user: the page explaining
/// accounts, passwords and API keys.
const AUTH_HELP_URL: &str =
    "https://github.com/radiosilence/koan/blob/main/docs/guide/authentication.md#subsonic-api";

/// Largest form body a POST may carry. `createPlaylist` repeats `songId` once
/// per track, so this is sized for playlists thousands long.
const MAX_FORM_BODY: usize = 1 << 20;

/// The OpenSubsonic extensions this server implements, and their versions.
const EXTENSIONS: &[(&str, &[i64])] = &[
    ("apiKeyAuthentication", &[1]),
    ("formPost", &[1]),
    ("songLyrics", &[1]),
    ("indexBasedQueue", &[1]),
    // koan's own. See `koan_core::remote::profile`.
    (koan_core::remote::profile::LINK, &[1]),
    (koan_core::remote::profile::DEVICES, &[1]),
    (koan_core::remote::profile::INVITE, &[1]),
    (koan_core::remote::profile::SHARES, &[1]),
    (koan_core::remote::profile::PAIR, &[1]),
    (koan_core::remote::profile::HISTORY, &[1]),
    (koan_core::remote::profile::SIGN_IN, &[1]),
    (koan_core::remote::profile::PASSWORDS, &[1]),
    (koan_core::remote::profile::API_KEYS, &[1]),
    (koan_core::remote::profile::ACK, &[1]),
    (koan_core::remote::profile::DEVICE_KEYS, &[1]),
    (koan_core::remote::profile::SCROBBLING, &[1]),
    (koan_core::remote::profile::DSP_PROFILES, &[1]),
    (koan_core::remote::profile::DSP_DELETED, &[1]),
    (koan_core::remote::profile::TV_PAIRING, &[1]),
    (koan_core::remote::profile::PLAYLIST_ORDER, &[1]),
    (koan_core::remote::profile::COVERS, &[1]),
    (koan_core::remote::profile::ARTIST_INFO, &[1]),
];

/// Articles clients strip when sorting the artist index. Every real server
/// sends this; DSub sorts wrongly without it.
const IGNORED_ARTICLES: &str = "The El La Los Las Le Les";

struct AppState {
    pool: Arc<Pool>,
    username: String,
    /// The `[subsonic]` shared secret; without one, only accounts sign in.
    password: Option<String>,
    users: Arc<PasswordVerifier>,
    /// What app passwords are sealed under; `None` until the server has a
    /// signing key, and with it no app passwords.
    app_key: Option<[u8; 32]>,
    /// What authenticator app secrets are sealed under, likewise.
    totp_key: Option<[u8; 32]>,
    /// Upstream Navidrome/Subsonic, used to build signed stream URLs for tracks
    /// with no local file. Resolved once at startup rather than per request,
    /// which would re-read two TOML files. Credentials rather than a
    /// `SubsonicClient`: that builds blocking `reqwest` clients, which panics
    /// when constructed inside the tokio runtime.
    upstream: Option<SubsonicAuth>,
    /// Async client for proxying those streams. `reqwest::Client` owns a
    /// connection pool, so it is built once and cloned.
    http: reqwest::Client,
    /// The web UI's cover cache, so every front end reads one set of renders.
    covers: Arc<crate::covers::Covers>,
    /// What `getIndexes`'s `lastModified` was last worked out from.
    last_modified: parking_lot::Mutex<Option<LibraryModified>>,
    /// `ffmpeg`, where transcoding is on and it was found at startup.
    transcoder: Option<crate::transcode::Transcoder>,
    /// `server.url`: where an assistant reaches `/mcp`, and what `ping` gives
    /// clients to know the server by.
    server_url: crate::ServerUrl,
    /// Whether `[push]` is enabled: the server pushes through the relay, and
    /// the app asks to show notifications only where it does.
    push: bool,
    /// Cancelled when the server stops, which ends every link.
    stop: tokio_util::sync::CancellationToken,
    /// Every link's session, which the server waits for as it stops.
    links: tokio_util::task::TaskTracker,
    /// An embedded server's key for the app hosting it: signs in as the first
    /// admin. See `ApiServer::host_key`.
    host_key: Option<String>,
}

/// When the library last changed, as far as this process has seen.
#[derive(Clone, Copy)]
struct LibraryModified {
    /// The library as `newest_file` was read from it.
    stamp: queries::LibraryStamp,
    /// The newest track file's mtime, in milliseconds.
    newest_file: i64,
    /// When the stamp's rows were last seen to change, in milliseconds. The
    /// first reading in a process counts as a change, since what changed while
    /// it was down cannot be known: a restart has clients walk once.
    changed: i64,
}

impl AppState {
    fn server_url(&self) -> Option<String> {
        self.server_url.get()
    }

    fn open_db(&self) -> Result<Handle<'_>, SubsonicError> {
        self.pool.get().map_err(SubsonicError::internal)
    }

    /// When the library last changed, in milliseconds: the later of the
    /// newest track file and the last time a track, album or artist was added
    /// or removed. Clients that keep a copy of the library — koan's own among
    /// them — compare it with what they last walked, so it has to move for a
    /// deletion too, which no file's mtime records.
    ///
    /// `MAX(mtime)` reads every track, and clients poll `getIndexes`. Read
    /// again when the library's stamp moves (`queries::library_stamp`).
    fn last_modified(&self, db: &Database) -> Result<i64, SubsonicError> {
        let stamp =
            queries::library_stamp(&db.conn).map_err(|e| SubsonicError::internal(e.to_string()))?;
        let held = *self.last_modified.lock();
        if let Some(h) = held
            && h.stamp == stamp
        {
            return Ok(h.newest_file.max(h.changed));
        }
        let newest_file: i64 = db
            .conn
            .query_row(
                "SELECT COALESCE(MAX(mtime), 0) * 1000 FROM tracks",
                [],
                |r| r.get(0),
            )
            .map_err(|e| SubsonicError::internal(e.to_string()))?;
        let changed = match held {
            Some(h) if h.stamp.rows == stamp.rows => h.changed,
            _ => koan_core::clock::unix_ms(),
        };
        *self.last_modified.lock() = Some(LibraryModified {
            stamp,
            newest_file,
            changed,
        });
        Ok(newest_file.max(changed))
    }
}

/// Register all Subsonic REST routes on the given router.
fn register_subsonic_routes(router: axum::Router<Arc<AppState>>) -> axum::Router<Arc<AppState>> {
    router
        // Browsing (ID3)
        .route("/rest/ping", get(ping).post(ping))
        // koan's own: a koan client's standing connection, for the server to
        // command it. See `crate::clients`.
        .route("/rest/koanLink", get(koan_link))
        // A device without a keyboard waiting to be signed in, and the
        // endpoints that sign it in. See `crate::pair`.
        .route("/rest/koanPair", get(crate::pair::route))
        .route(
            "/rest/koanPairInfo",
            get(koan_pair_info).post(koan_pair_info),
        )
        .route(
            "/rest/koanPairApprove",
            get(koan_pair_approve).post(koan_pair_approve),
        )
        .route("/rest/koanUsers", get(koan_users).post(koan_users))
        .route(
            "/rest/koanCreateUser",
            get(koan_create_user).post(koan_create_user),
        )
        .route("/rest/koanInvite", get(koan_invite).post(koan_invite))
        .route("/rest/koanApiKeys", get(koan_api_keys).post(koan_api_keys))
        .route("/rest/koanMcp", get(koan_mcp).post(koan_mcp))
        .route("/rest/koanCreateApiKey", post(koan_create_api_key))
        .route("/rest/koanRevokeApiKey", post(koan_revoke_api_key))
        .route(
            "/rest/koanAppPasswords",
            get(koan_app_passwords).post(koan_app_passwords),
        )
        .route(
            "/rest/koanCreateAppPassword",
            post(koan_create_app_password),
        )
        .route(
            "/rest/koanRevokeAppPassword",
            post(koan_revoke_app_password),
        )
        .route("/rest/koanSetUserPassword", post(koan_set_user_password))
        .route("/rest/koanJoin", get(koan_join).post(koan_join))
        .route("/rest/koanSignIn", get(koan_sign_in).post(koan_sign_in))
        .route(
            "/rest/koanRevokeKey",
            get(koan_revoke_key).post(koan_revoke_key),
        )
        .route(
            "/rest/koanSetUserRole",
            get(koan_set_user_role).post(koan_set_user_role),
        )
        .route(
            "/rest/koanDeleteUser",
            get(koan_delete_user).post(koan_delete_user),
        )
        .route("/rest/koanCommand", get(koan_command).post(koan_command))
        .route("/rest/koanHistory", get(koan_history).post(koan_history))
        .route(
            "/rest/koanHistory.view",
            get(koan_history).post(koan_history),
        )
        .route(
            "/rest/koanForgetPlays",
            get(koan_forget_plays).post(koan_forget_plays),
        )
        .route(
            "/rest/koanForgetPlays.view",
            get(koan_forget_plays).post(koan_forget_plays),
        )
        .route(
            "/rest/koanScrobbling",
            get(koan_scrobbling).post(koan_scrobbling),
        )
        .route("/rest/koanScrobblingConnect", post(koan_scrobbling_connect))
        .route(
            "/rest/koanDspProfiles",
            get(koan_dsp_profiles).post(koan_dsp_profiles),
        )
        .route("/rest/koanDspProfileSave", post(koan_dsp_profile_save))
        .route("/rest/koanDspProfileDelete", post(koan_dsp_profile_delete))
        .route("/rest/koanDspDismiss", post(koan_dsp_dismiss))
        .route(
            "/rest/koanPlaylistOrder",
            get(koan_playlist_order).post(koan_playlist_order),
        )
        .route("/rest/koanCovers", get(koan_covers).post(koan_covers))
        .route(
            "/rest/koanDspDeleted",
            get(koan_dsp_deleted).post(koan_dsp_deleted),
        )
        .route("/rest/koanDspRestore", post(koan_dsp_restore))
        .route(
            "/rest/koanDspFile",
            get(koan_dsp_file).post(koan_dsp_file_upload),
        )
        .route(
            "/rest/koanScrobblingDisconnect",
            get(koan_scrobbling_disconnect).post(koan_scrobbling_disconnect),
        )
        .route(
            "/rest/koanCommand.view",
            get(koan_command).post(koan_command),
        )
        .route("/rest/ping.view", get(ping).post(ping))
        // Sharing
        .route("/rest/createShare", get(create_share).post(create_share))
        .route(
            "/rest/createShare.view",
            get(create_share).post(create_share),
        )
        .route("/rest/getShares", get(get_shares).post(get_shares))
        .route("/rest/getShares.view", get(get_shares).post(get_shares))
        .route("/rest/updateShare", get(update_share).post(update_share))
        .route(
            "/rest/updateShare.view",
            get(update_share).post(update_share),
        )
        .route("/rest/deleteShare", get(delete_share).post(delete_share))
        .route(
            "/rest/deleteShare.view",
            get(delete_share).post(delete_share),
        )
        .route(
            "/rest/getOpenSubsonicExtensions",
            get(get_open_subsonic_extensions).post(get_open_subsonic_extensions),
        )
        .route(
            "/rest/getOpenSubsonicExtensions.view",
            get(get_open_subsonic_extensions).post(get_open_subsonic_extensions),
        )
        .route("/rest/tokenInfo", get(token_info).post(token_info))
        .route("/rest/tokenInfo.view", get(token_info).post(token_info))
        .route(
            "/rest/getLyricsBySongId",
            get(get_lyrics_by_song_id).post(get_lyrics_by_song_id),
        )
        .route(
            "/rest/getLyricsBySongId.view",
            get(get_lyrics_by_song_id).post(get_lyrics_by_song_id),
        )
        .route("/rest/getLicense", get(get_license).post(get_license))
        .route("/rest/getLicense.view", get(get_license).post(get_license))
        .route("/rest/getArtists", get(get_artists).post(get_artists))
        .route("/rest/getArtists.view", get(get_artists).post(get_artists))
        .route("/rest/getArtist", get(get_artist).post(get_artist))
        .route("/rest/getArtist.view", get(get_artist).post(get_artist))
        .route(
            "/rest/getArtistInfo2",
            get(get_artist_info2).post(get_artist_info2),
        )
        .route(
            "/rest/getArtistInfo2.view",
            get(get_artist_info2).post(get_artist_info2),
        )
        .route(
            "/rest/koanArtistImage",
            get(koan_artist_image).post(koan_artist_image),
        )
        .route("/rest/getAlbum", get(get_album).post(get_album))
        .route("/rest/getAlbum.view", get(get_album).post(get_album))
        .route(
            "/rest/getAlbumList",
            get(get_album_list).post(get_album_list),
        )
        .route(
            "/rest/getAlbumList.view",
            get(get_album_list).post(get_album_list),
        )
        .route(
            "/rest/getAlbumList2",
            get(get_album_list2).post(get_album_list2),
        )
        .route(
            "/rest/getAlbumList2.view",
            get(get_album_list2).post(get_album_list2),
        )
        .route("/rest/getSong", get(get_song).post(get_song))
        .route("/rest/getSong.view", get(get_song).post(get_song))
        // Browsing (file tree)
        .route("/rest/getIndexes", get(get_indexes).post(get_indexes))
        .route("/rest/getIndexes.view", get(get_indexes).post(get_indexes))
        .route(
            "/rest/getMusicDirectory",
            get(get_music_directory).post(get_music_directory),
        )
        .route(
            "/rest/getMusicDirectory.view",
            get(get_music_directory).post(get_music_directory),
        )
        // Search
        .route("/rest/search3", get(search3).post(search3))
        .route("/rest/search3.view", get(search3).post(search3))
        // Streaming + media
        .route("/rest/stream", get(stream).post(stream))
        .route("/rest/stream.view", get(stream).post(stream))
        // `download` is the untranscoded original, which is all `stream` ever
        // serves here. koan's own download queue fetches through it.
        .route("/rest/download", get(download).post(download))
        .route("/rest/download.view", get(download).post(download))
        .route("/rest/getCoverArt", get(get_cover_art).post(get_cover_art))
        .route(
            "/rest/getCoverArt.view",
            get(get_cover_art).post(get_cover_art),
        )
        // Interaction
        .route("/rest/star", get(star).post(star))
        .route("/rest/star.view", get(star).post(star))
        .route("/rest/unstar", get(unstar).post(unstar))
        .route("/rest/unstar.view", get(unstar).post(unstar))
        .route("/rest/getStarred2", get(get_starred2).post(get_starred2))
        .route(
            "/rest/getStarred2.view",
            get(get_starred2).post(get_starred2),
        )
        .route(
            "/rest/getPlayQueue",
            get(get_play_queue).post(get_play_queue),
        )
        .route(
            "/rest/getPlayQueue.view",
            get(get_play_queue).post(get_play_queue),
        )
        .route(
            "/rest/savePlayQueue",
            get(save_play_queue).post(save_play_queue),
        )
        .route(
            "/rest/savePlayQueue.view",
            get(save_play_queue).post(save_play_queue),
        )
        .route(
            "/rest/getPlayQueueByIndex",
            get(get_play_queue_by_index).post(get_play_queue_by_index),
        )
        .route(
            "/rest/getPlayQueueByIndex.view",
            get(get_play_queue_by_index).post(get_play_queue_by_index),
        )
        .route(
            "/rest/savePlayQueueByIndex",
            get(save_play_queue_by_index).post(save_play_queue_by_index),
        )
        .route(
            "/rest/savePlayQueueByIndex.view",
            get(save_play_queue_by_index).post(save_play_queue_by_index),
        )
        .route("/rest/getBookmarks", get(get_bookmarks).post(get_bookmarks))
        .route(
            "/rest/getBookmarks.view",
            get(get_bookmarks).post(get_bookmarks),
        )
        .route(
            "/rest/createBookmark",
            get(create_bookmark).post(create_bookmark),
        )
        .route(
            "/rest/createBookmark.view",
            get(create_bookmark).post(create_bookmark),
        )
        .route(
            "/rest/deleteBookmark",
            get(delete_bookmark).post(delete_bookmark),
        )
        .route(
            "/rest/deleteBookmark.view",
            get(delete_bookmark).post(delete_bookmark),
        )
        .route("/rest/setRating", get(set_rating).post(set_rating))
        .route("/rest/setRating.view", get(set_rating).post(set_rating))
        .route("/rest/scrobble", get(scrobble).post(scrobble))
        .route("/rest/scrobble.view", get(scrobble).post(scrobble))
        .route(
            "/rest/getRandomSongs",
            get(get_random_songs).post(get_random_songs),
        )
        .route(
            "/rest/getRandomSongs.view",
            get(get_random_songs).post(get_random_songs),
        )
        // Server + user metadata
        .route(
            "/rest/getMusicFolders",
            get(get_music_folders).post(get_music_folders),
        )
        .route(
            "/rest/getMusicFolders.view",
            get(get_music_folders).post(get_music_folders),
        )
        .route("/rest/getGenres", get(get_genres).post(get_genres))
        .route("/rest/getGenres.view", get(get_genres).post(get_genres))
        .route("/rest/getUser", get(get_user).post(get_user))
        .route("/rest/getUser.view", get(get_user).post(get_user))
        .route(
            "/rest/getScanStatus",
            get(get_scan_status).post(get_scan_status),
        )
        .route(
            "/rest/getScanStatus.view",
            get(get_scan_status).post(get_scan_status),
        )
        // Playlists
        .route("/rest/getPlaylists", get(get_playlists).post(get_playlists))
        .route(
            "/rest/getPlaylists.view",
            get(get_playlists).post(get_playlists),
        )
        .route("/rest/getPlaylist", get(get_playlist).post(get_playlist))
        .route(
            "/rest/getPlaylist.view",
            get(get_playlist).post(get_playlist),
        )
        .route(
            "/rest/createPlaylist",
            get(create_playlist).post(create_playlist),
        )
        .route(
            "/rest/createPlaylist.view",
            get(create_playlist).post(create_playlist),
        )
        .route(
            "/rest/updatePlaylist",
            get(update_playlist).post(update_playlist),
        )
        .route(
            "/rest/updatePlaylist.view",
            get(update_playlist).post(update_playlist),
        )
        .route(
            "/rest/deletePlaylist",
            get(delete_playlist).post(delete_playlist),
        )
        .route(
            "/rest/deletePlaylist.view",
            get(delete_playlist).post(delete_playlist),
        )
        // Everything else under /rest/
        .route(
            "/rest/{*endpoint}",
            get(unsupported_endpoint).post(unsupported_endpoint),
        )
}

/// The Subsonic routes with their state and the layers every request passes.
/// `form_post` is outermost, so its [`FormBody`] is in place when the sign-in
/// throttle reads the request's parameters, which it does through
/// [`request_params`] as the handlers do: credentials sent in a form body are
/// throttled, and an API key sent in one passes, as in the query.
fn subsonic_app(state: Arc<AppState>) -> axum::Router {
    let throttle = Arc::new(AuthThrottle::new(
        state.password.is_some().then(|| state.username.clone()),
        state.users.clone(),
    ));
    // For the routes that hold a socket and take no state, as pairing does.
    let stop = axum::Extension(state.stop.clone());
    register_subsonic_routes(axum::Router::new())
        .with_state(state)
        .layer(stop)
        .layer(axum::middleware::from_fn_with_state(
            throttle,
            throttle_auth,
        ))
        .layer(axum::middleware::from_fn(form_post))
}

/// Build a Subsonic-compatible REST API router.
///
/// Returns `None` unless `[subsonic]` is enabled. Without a shared secret only
/// koan accounts sign in. `/rest/*` carries no JWT layer, so these credentials
/// alone guard every byte of the library — the secret must never be the
/// upstream `[remote]` password.
pub fn subsonic_router(
    pool: Arc<Pool>,
    covers: Arc<crate::covers::Covers>,
    users: Arc<PasswordVerifier>,
    profile: crate::graphql::Profile,
    stop: tokio_util::sync::CancellationToken,
    links: tokio_util::task::TaskTracker,
    host_key: Option<String>,
) -> axum::Router {
    let cfg = Config::cached();

    // The shared secret belongs to a deployment that chose it; an app sharing
    // its library signs in accounts only.
    let password = koan_core::helpers::get_subsonic_password(&cfg)
        .filter(|_| !cfg.subsonic.username.is_empty())
        .filter(|_| profile != crate::graphql::Profile::Embedded);
    if password.is_none() {
        log::info!("Subsonic: no shared secret, so only koan accounts sign in (with p=).");
    }

    let transcoder = cfg
        .subsonic
        .transcode
        .then(|| {
            let found = crate::transcode::Transcoder::find(&cfg.subsonic.ffmpeg);
            if found.is_none() {
                log::info!(
                    "Subsonic: {} did not run, so clients asking for a lower bitrate get the original.",
                    cfg.subsonic.ffmpeg
                );
            }
            found
        })
        .flatten();

    let state = Arc::new(AppState {
        users,
        pool,
        username: cfg.subsonic.username.clone(),
        password,
        app_key: koan_core::auth::load_keypair()
            .ok()
            .map(|(private, _)| koan_core::auth::app_password_key(&private)),
        totp_key: koan_core::auth::load_keypair()
            .ok()
            .map(|(private, _)| koan_auth::second_factor::totp_key(&private)),
        upstream: koan_core::helpers::subsonic_auth(&cfg),
        http: reqwest::Client::builder()
            // A whole-request deadline would cut off long proxied streams.
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default(),
        covers,
        last_modified: Default::default(),
        transcoder,
        push: crate::push::pusher().is_some(),
        server_url: match profile {
            crate::graphql::Profile::Embedded => crate::ServerUrl::Live,
            _ => crate::ServerUrl::Fixed(cfg.server_url()),
        },
        stop: stop.clone(),
        links,
        host_key,
    });
    spawn_key_publisher(state.pool.clone(), stop);

    subsonic_app(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    /// Sequential requests, the shape of a client syncing a library. Ignored:
    /// a timing to read, not an assertion. `cargo test -p koan-server --release
    /// -- --ignored --nocapture sequential_request_timing`.
    #[tokio::test]
    #[ignore]
    async fn sequential_request_timing() {
        const N: usize = 500;
        let (state, dir) = test_state();
        seed_data(&state);
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        let album = queries::all_albums(&db.conn).unwrap()[0].id;
        drop(db);
        let app = build_test_router(state);

        for (label, auth) in [
            ("shared secret", auth_query("")),
            ("account", "u=mate&p=hunter22&v=1.16.1&c=test".to_string()),
        ] {
            let uri = format!("/rest/getAlbum?{auth}&id={album}");
            let (status, body) = get_response(app.clone(), &uri).await;
            assert_eq!(status, StatusCode::OK);
            assert!(body.contains("Test Album"), "{body}");
            let start = std::time::Instant::now();
            for _ in 0..N {
                get_response(app.clone(), &uri).await;
            }
            let elapsed = start.elapsed();
            println!(
                "{N} x getAlbum ({label}): {elapsed:?}, {:?} per request",
                elapsed / N as u32
            );
        }
    }

    /// A route that needs no database stays fast while slow Subsonic requests
    /// are in flight — argon2 on a wrong password, which is never remembered.
    /// With that work on the runtime's two workers, the first probe waited
    /// over a second; off them it takes milliseconds. The bound leaves room
    /// for a loaded CI machine and still fails the regression.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trivial_route_under_load() {
        const SLOW: usize = 8;
        const PROBES: usize = 20;
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let slow_uri = "/rest/getAlbum?u=mate&p=wrong&v=1.16.1&c=test&id=1";
        let probe_uri = "/rest/getOpenSubsonicExtensions";

        let probe = app.clone();
        let idle = tokio::spawn(async move {
            let t = std::time::Instant::now();
            get_response(probe, probe_uri).await;
            t.elapsed()
        })
        .await
        .unwrap();

        let started = std::time::Instant::now();
        let slow: Vec<_> = (0..SLOW)
            .map(|_| {
                let app = app.clone();
                tokio::spawn(async move { get_response(app, slow_uri).await })
            })
            .collect();
        // The test body does not run on a worker, so each probe is spawned and
        // timed from the spawn, as an arriving request would be. A thread sleep
        // lets the slow requests take the workers; a timer would need one to fire.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut probes = Vec::with_capacity(PROBES);
        for _ in 0..PROBES {
            let t = std::time::Instant::now();
            let (status, _) = tokio::spawn(get_response(app.clone(), probe_uri))
                .await
                .unwrap();
            assert_eq!(status, StatusCode::OK);
            probes.push(t.elapsed());
        }
        for s in slow {
            s.await.unwrap();
        }
        let slow_total = started.elapsed();
        probes.sort();
        println!(
            "idle probe {idle:?}; under {SLOW} slow requests: median {:?}, max {:?}; \
             slow requests done in {slow_total:?}",
            probes[PROBES / 2],
            probes[PROBES - 1],
        );
        assert!(
            probes[PROBES - 1] < std::time::Duration::from_millis(750),
            "a trivial route waited {:?} behind slow Subsonic requests",
            probes[PROBES - 1]
        );
    }

    #[tokio::test]
    async fn test_view_suffix_routes() {
        let (state, _dir) = test_state();
        let app = build_test_router(state);
        let (status, body) =
            get_response(app, &format!("/rest/ping.view?{}", auth_query(""))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("status=\"ok\""));
    }
}

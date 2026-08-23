//! Shared helpers used by downstream crates (koan-tui, koan-server, koan-cli).
//!
//! These functions provide common functionality for building playlist items,
//! resolving track paths, downloading remote tracks, and building Subsonic clients.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::player::commands::PlayerCommand;
use crate::player::state::{LoadState, PlaylistItem, QueueItemId, SharedPlayerState};
use crate::remote::client::SubsonicClient;

// ---------------------------------------------------------------------------
// Subsonic client builder
// ---------------------------------------------------------------------------

/// Get the remote password from config, falling back to Keychain for backwards compat.
pub fn get_remote_password(cfg: &Config) -> Option<String> {
    if !cfg.remote.password.is_empty() {
        return Some(cfg.remote.password.clone());
    }
    // Fallback to Keychain for users who set up before the config change.
    crate::credentials::get_password(&cfg.remote.url).ok()
}

/// Keychain account holding the Subsonic API shared secret.
pub const SUBSONIC_CREDENTIAL_ACCOUNT: &str = "koan-subsonic";

/// Shared secret for koan's own Subsonic API. Config first, then the keychain.
///
/// Deliberately not `get_remote_password` — see `SubsonicConfig`.
pub fn get_subsonic_password(cfg: &Config) -> Option<String> {
    if !cfg.subsonic.password.is_empty() {
        return Some(cfg.subsonic.password.clone());
    }
    crate::credentials::get_password(SUBSONIC_CREDENTIAL_ACCOUNT)
        .ok()
        .filter(|p| !p.is_empty())
}

/// Build a `SubsonicClient` from the merged config, returning `None` if remote
/// is disabled or has no URL configured.
pub fn subsonic_client(cfg: &Config) -> Option<SubsonicClient> {
    if !cfg.remote.enabled || cfg.remote.url.is_empty() {
        return None;
    }
    let password = get_remote_password(cfg)?;
    Some(SubsonicClient::new(
        &cfg.remote.url,
        &cfg.remote.username,
        &password,
    ))
}

// ---------------------------------------------------------------------------
// Sharing
// ---------------------------------------------------------------------------

/// Why a share link could not be made. Each variant is something the user can
/// act on, which is the point — every caller used to collapse these into
/// "local-only tracks can't be shared" and send people looking in the wrong
/// place.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error("no remote server is configured")]
    NoRemote,
    #[error("none of these tracks are on the server, so a link has nothing to point at")]
    NothingRemote,
    #[error("the server refused to share these: {0}")]
    Server(#[from] crate::remote::client::SubsonicError),
    #[error(transparent)]
    Database(#[from] crate::db::connection::DbError),
}

/// A created share link, and how much of the request it covers.
#[derive(Debug, Clone)]
pub struct ShareOutcome {
    pub url: String,
    /// The server's own ID for the share, for callers that manage them.
    pub id: String,
    /// Tracks the server knows about, which went into the link.
    pub shared: usize,
    /// Tracks with no copy on the server, left out of it.
    pub skipped: usize,
}

/// Create a public share link on the remote server for these tracks.
///
/// A link points at the server, so only tracks the server knows about can go in
/// it. A mixed selection shares the part that can be shared and reports the
/// rest rather than failing whole — half a link beats none, as long as the
/// caller says which half.
///
/// Network-bound. Callers keep it off whatever thread draws.
pub fn create_share(
    db: &Database,
    cfg: &Config,
    track_ids: &[i64],
    description: Option<&str>,
) -> Result<ShareOutcome, ShareError> {
    let client = subsonic_client(cfg).ok_or(ShareError::NoRemote)?;

    // One query, not one per track: sharing an artist is thousands of tracks.
    let rows = queries::tracks_by_ids(&db.conn, track_ids)?;

    // Tracks koan holds no remote ID for are not necessarily absent from the
    // server — see below.
    let mut resolved: Vec<Option<String>> = rows.iter().map(|t| t.remote_id.clone()).collect();
    let missing: Vec<usize> = resolved
        .iter()
        .enumerate()
        .filter(|(_, rid)| rid.is_none())
        .map(|(i, _)| i)
        .take(RESOLVE_LIMIT)
        .collect();
    for i in missing {
        resolved[i] = identify_on_server(&client, &rows[i]);
    }

    let shared = resolved.iter().filter(|r| r.is_some()).count();
    if shared == 0 {
        return Err(ShareError::NothingRemote);
    }

    // A whole record shares as one album rather than as N tracks — the server
    // renders it as the album it is, and the link survives the user adding to
    // it. Only when the selection is genuinely the whole thing.
    let one_album = rows
        .first()
        .and_then(|f| f.album_id)
        .filter(|first| rows.iter().all(|t| t.album_id == Some(*first)))
        .and_then(|album_id| album_remote_id(&db.conn, album_id, rows.len()));

    let remote_ids: Vec<String> = match one_album {
        Some(rid) => vec![rid],
        None => resolved.into_iter().flatten().collect(),
    };

    let refs: Vec<&str> = remote_ids.iter().map(String::as_str).collect();
    let share = client.create_share(&refs, description)?;

    // Navidrome does not always hand back a URL, and a share with no link is
    // useless to the caller — the ID is enough to build it.
    let url = share
        .url
        .clone()
        .unwrap_or_else(|| format!("{}/s/{}", client.base_url(), share.id));

    Ok(ShareOutcome {
        url,
        id: share.id,
        shared,
        skipped: track_ids.len().saturating_sub(shared),
    })
}

/// How many unidentified tracks one share will ask the server about. Each is a
/// round trip, and a share of a whole artist could otherwise be hundreds.
const RESOLVE_LIMIT: usize = 50;

/// Ask the server to identify a track koan holds no remote ID for.
///
/// A local file and its copy on the server fail to merge whenever the two
/// disagree about naming — a truncated ID3v1 title, a `(Deluxe)` suffix, a
/// reissue (#221) — which leaves a record that is plainly on the server looking
/// local-only, and unshareable. Rather than guess at the naming, ask the server:
/// its own search is better at this than any heuristic here would be.
///
/// A hit counts only when exactly one song comes back matching on artist and
/// duration, so an ambiguous answer leaves the track out rather than sharing the
/// wrong one.
///
/// The result is deliberately not written back. Two rows would then carry the
/// same `remote_id`, which is a duplicate rather than the merge #221 actually
/// wants — repairing the rows properly means reconciling favourites and play
/// counts too, which is that issue's job, not this one's.
fn identify_on_server(client: &SubsonicClient, track: &queries::TrackRow) -> Option<String> {
    let hits = client.search(&track.title).ok()?;
    sole_match(&hits.song, &track.artist_name, track.duration_ms)
}

/// The one song that can only be this track. `None` when nothing matches, and
/// equally when more than one does — an ambiguous answer leaves the track out
/// rather than sharing the wrong recording.
fn sole_match(
    songs: &[crate::remote::client::SubsonicSong],
    artist: &str,
    duration_ms: Option<i64>,
) -> Option<String> {
    let mut matches = songs.iter().filter(|song| {
        song.artist
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case(artist))
            && match (song.duration, duration_ms) {
                (Some(secs), Some(ms)) => (secs * 1000 - ms).abs() < 2000,
                _ => false,
            }
    });
    let first = matches.next()?;
    matches.next().is_none().then(|| first.id.clone())
}

/// The album's own remote ID, but only when `selected` covers every track on
/// it. Sharing an album link for half an album would hand out more than the
/// user picked.
fn album_remote_id(conn: &rusqlite::Connection, album_id: i64, selected: usize) -> Option<String> {
    let (remote_id, total): (Option<String>, i64) = conn
        .query_row(
            "SELECT al.remote_id, (SELECT COUNT(*) FROM tracks WHERE album_id = al.id)
             FROM albums al WHERE al.id = ?1",
            [album_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok()?;
    (total == selected as i64).then_some(remote_id).flatten()
}

// ---------------------------------------------------------------------------
// Path utilities
// ---------------------------------------------------------------------------

/// Truncate a string to at most `max` bytes, cutting on a char boundary.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Sanitise and truncate a string for use as a path component.
/// Strips illegal chars and caps at 240 bytes (macOS 255-byte filename limit minus room for ext).
pub fn sanitise_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string();

    truncate_bytes(&cleaned, 240).trim_end().to_string()
}

/// Build a structured cache path for a track:
///   cache_dir/Album Artist/(Year) Album [Codec]/01. Track Artist - Title.ext
pub fn cache_path_for_track(
    cache_dir: &Path,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> PathBuf {
    let artist_dir = sanitise_filename(&track.artist_name);

    let year = album_date
        .and_then(|d| if d.len() >= 4 { Some(&d[..4]) } else { None })
        .map(|y| format!("({}) ", y))
        .unwrap_or_default();
    let codec = track
        .codec
        .as_deref()
        .map(|c| format!(" [{}]", c))
        .unwrap_or_default();
    let album_dir = sanitise_filename(&format!("{}{}{}", year, track.album_title, codec));

    let disc_prefix = match track.disc {
        Some(d) if d > 1 => format!("{}-", d),
        _ => String::new(),
    };
    let track_num = track
        .track_number
        .map(|n| format!("{:02}. ", n))
        .unwrap_or_default();

    let ext = track
        .codec
        .as_deref()
        .map(|c| c.to_lowercase())
        .unwrap_or_else(|| "flac".into());

    let filename = sanitise_filename(&format!(
        "{}{}{} - {}",
        disc_prefix, track_num, track.artist_name, track.title
    ));

    cache_dir
        .join(artist_dir)
        .join(album_dir)
        .join(format!("{}.{}", filename, ext))
}

// ---------------------------------------------------------------------------
// Track resolution
// ---------------------------------------------------------------------------

/// Resolve a track to its path + load state (without downloading).
/// Returns (path, LoadState::Ready) for local/cached, (cache_path, LoadState::Pending) for remote.
pub fn resolve_item_path(
    db: &Database,
    cfg: &Config,
    id: i64,
    track: &queries::TrackRow,
    album_date: Option<&str>,
) -> (PathBuf, LoadState) {
    match queries::resolve_playback_path(&db.conn, id) {
        Ok(Some(queries::PlaybackSource::Local(p))) => (p, LoadState::Ready),
        // A cache entry is only as good as its contents. Older builds could
        // store a Subsonic error body here, which reports Ready and then fails
        // to decode forever; treating it as Pending sends it back through the
        // download path, which discards it and re-fetches.
        Ok(Some(queries::PlaybackSource::Cached(p))) => {
            let state = if is_cached_audio(&p) {
                LoadState::Ready
            } else {
                LoadState::Pending
            };
            (p, state)
        }
        Ok(Some(queries::PlaybackSource::Remote(_))) => {
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            if dest.exists() && is_cached_audio(&dest) {
                (dest, LoadState::Ready)
            } else {
                (dest, LoadState::Pending)
            }
        }
        _ => {
            // Fallback: construct a cache path and mark pending.
            let dest = cache_path_for_track(&cfg.cache_dir(), track, album_date);
            (dest, LoadState::Pending)
        }
    }
}

/// Build a PlaylistItem from a TrackRow + album date + resolved path + load state.
pub fn playlist_item_from_track(
    track: &queries::TrackRow,
    album_date: Option<&str>,
    dest: PathBuf,
    load_state: LoadState,
) -> PlaylistItem {
    let year = album_date.and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });
    PlaylistItem {
        id: QueueItemId::new(),
        db_id: Some(track.id),
        path: dest,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        load_state,
    }
}

/// Build playlist items for many tracks at once.
///
/// `track_to_playlist_item` loads the config on every call, which means
/// reading and parsing `config.toml` and `config.local.toml` once per track —
/// the reason a large add crawled. This loads it once and memoises album dates,
/// so a thousand-track add costs one config read instead of a thousand.
pub fn playlist_items_for_tracks(db: &Database, tracks: &[queries::TrackRow]) -> Vec<PlaylistItem> {
    use std::collections::HashMap;

    let cfg = Config::load().unwrap_or_default();
    let mut album_dates: HashMap<i64, Option<String>> = HashMap::new();

    tracks
        .iter()
        .map(|track| {
            let album_date = match track.album_id {
                Some(aid) => album_dates
                    .entry(aid)
                    .or_insert_with(|| queries::album_date(&db.conn, aid).ok().flatten())
                    .clone(),
                None => None,
            };
            let (path, load_state) =
                resolve_item_path(db, &cfg, track.id, track, album_date.as_deref());
            playlist_item_from_track(track, album_date.as_deref(), path, load_state)
        })
        .collect()
}

/// Build a PlaylistItem from a TrackRow, resolving its path automatically.
pub fn track_to_playlist_item(track: &queries::TrackRow, db: &Database) -> PlaylistItem {
    let album_date = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let cfg = Config::load().unwrap_or_default();
    let (path, load_state) = resolve_item_path(db, &cfg, track.id, track, album_date.as_deref());

    let year = album_date.as_deref().and_then(|d| {
        if d.len() >= 4 {
            Some(d[..4].to_string())
        } else {
            None
        }
    });

    PlaylistItem {
        id: QueueItemId::new(),
        db_id: Some(track.id),
        path,
        title: track.title.clone(),
        artist: track.artist_name.clone(),
        album_artist: track.album_artist_name.clone(),
        album: track.album_title.clone(),
        year,
        codec: track.codec.clone(),
        track_number: track.track_number.map(|n| n as i64),
        disc: track.disc.map(|n| n as i64),
        duration_ms: track.duration_ms.map(|d| d as u64),
        load_state,
    }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Whether a cached file plausibly holds audio.
///
/// A stored Subsonic error is a few hundred bytes of JSON or XML; no real
/// encoded track comes close to that, so the size check alone settles almost
/// every case and the leading byte covers the rest.
fn is_cached_audio(path: &std::path::Path) -> bool {
    const MIN_PLAUSIBLE_BYTES: u64 = 4096;
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() >= MIN_PLAUSIBLE_BYTES => true,
        Ok(_) => {
            let mut first = [0u8; 1];
            match std::fs::File::open(path)
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut first).map(|_| first[0]))
            {
                Ok(b) => b != b'{' && b != b'<',
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

/// Resolve a track to a playable file, downloading from remote if needed.
///
/// Resolution order:
/// 1. Local library path (DB `path` field) -- use directly if file exists
/// 2. Cache path -- use if already downloaded
/// 3. Download from remote to cache -- stream while downloading
pub fn download_track(
    db_id: i64,
    queue_id: QueueItemId,
    tx: &crossbeam_channel::Sender<PlayerCommand>,
    log_buf: &Arc<Mutex<Vec<String>>>,
    state: &Arc<SharedPlayerState>,
    cfg: &Config,
    client: &SubsonicClient,
) {
    let db = match Database::open_default() {
        Ok(db) => db,
        Err(e) => {
            state.update_load_state(queue_id, LoadState::Failed(format!("db error: {}", e)));
            return;
        }
    };
    let track = match queries::get_track_row(&db.conn, db_id) {
        Ok(Some(t)) => t,
        _ => {
            state.update_load_state(queue_id, LoadState::Failed("track not found".into()));
            return;
        }
    };

    let remote_id = match &track.remote_id {
        Some(rid) => rid.clone(),
        None => {
            // No remote_id -- check if the local file exists.
            if let Some(ref path) = track.path {
                let p = std::path::PathBuf::from(path);
                if p.exists() {
                    state.update_paths(&[(queue_id, p)]);
                    state.update_load_state(queue_id, LoadState::Ready);
                    if state.is_cursor(queue_id) {
                        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
                    }
                    return;
                }
            }
            state.update_load_state(queue_id, LoadState::Failed("no remote_id".into()));
            return;
        }
    };

    // 1. Check if the local library file exists.
    if let Some(ref local_path) = track.path {
        let p = std::path::PathBuf::from(local_path);
        if p.exists() {
            log::info!("download_track: local file exists, using {}", p.display());
            state.update_paths(&[(queue_id, p)]);
            state.update_load_state(queue_id, LoadState::Ready);
            if state.is_cursor(queue_id) {
                tx.send(PlayerCommand::TrackReady(queue_id)).ok();
            }
            return;
        }
    }

    let album_date: Option<String> = track
        .album_id
        .and_then(|aid| queries::album_date(&db.conn, aid).ok().flatten());

    let dest = cache_path_for_track(&cfg.cache_dir(), &track, album_date.as_deref());

    // 2. Already cached.
    //
    // Older builds could write a Subsonic error body here as if it were audio,
    // leaving a tiny JSON file that reports Ready and then fails to decode
    // forever. Treat those as absent so they get re-fetched.
    if dest.exists() && !is_cached_audio(&dest) {
        log::warn!(
            "discarding non-audio cache entry {} (likely a stored server error)",
            dest.display()
        );
        let _ = std::fs::remove_file(&dest);
    }
    if dest.exists() {
        state.update_paths(&[(queue_id, dest)]);
        state.update_load_state(queue_id, LoadState::Ready);
        if state.is_cursor(queue_id) {
            tx.send(PlayerCommand::TrackReady(queue_id)).ok();
        }
        return;
    }

    // 3. Download from remote. The queue item points at the in-progress file so
    // the streaming pump reads bytes as they land; it flips to `dest` on success.
    state.update_paths(&[(queue_id, crate::remote::download::part_path(&dest))]);

    let bytes_written: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

    let progress_state = state.clone();
    let progress_qid = queue_id;
    let bytes_written_progress = bytes_written.clone();
    let progress_tx = tx.clone();
    let stream_ready_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stream_ready_flag = stream_ready_sent.clone();
    let result = client.download_with_progress(&remote_id, &dest, move |downloaded, total| {
        bytes_written_progress.store(downloaded, Ordering::Release);
        progress_state.update_load_state(
            progress_qid,
            LoadState::Downloading {
                downloaded,
                total,
                bytes_written: bytes_written_progress.clone(),
            },
        );
        if !stream_ready_flag.load(Ordering::Relaxed)
            && downloaded >= crate::player::state::STREAM_THRESHOLD
        {
            stream_ready_flag.store(true, Ordering::Relaxed);
            progress_tx
                .send(PlayerCommand::TrackStreamReady(progress_qid))
                .ok();
        }
    });

    if let Err(e) = result {
        state.update_load_state(queue_id, LoadState::Failed(e.to_string()));
        push_log(log_buf, format!("x {} — {}", track.title, e));
        return;
    }

    // Download succeeded.
    state.update_paths(&[(queue_id, dest.clone())]);
    state.update_load_state(queue_id, LoadState::Ready);
    // Without this row the file is invisible to cache eviction and never reclaimed.
    if let Err(e) = queries::set_cached_path(&db.conn, db_id, &dest.to_string_lossy()) {
        log::warn!(
            "cached {} but failed to record it ({}) — it will not be evicted",
            dest.display(),
            e
        );
    }

    push_log(
        log_buf,
        format!("+ {} — {}", track.title, track.artist_name),
    );

    if state.is_cursor(queue_id) {
        tx.send(PlayerCommand::TrackReady(queue_id)).ok();
    }
}

/// Append to the TUI log pane, tolerating a poisoned lock — a download worker
/// must not die because some other thread panicked while holding it.
fn push_log(log_buf: &Arc<Mutex<Vec<String>>>, msg: String) {
    match log_buf.lock() {
        Ok(mut buf) => buf.push(msg),
        Err(_) => log::info!("{}", msg),
    }
}

/// Spawn background downloads for remote tracks with LoadState::Pending.
pub fn spawn_downloads(
    pending: Vec<(i64, QueueItemId)>,
    tx: crossbeam_channel::Sender<PlayerCommand>,
    state: Arc<SharedPlayerState>,
) {
    let log_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    if let Err(e) = std::thread::Builder::new()
        .name("koan-download".into())
        .spawn(move || {
            let cfg = Config::load().unwrap_or_default();
            let Some(client) = subsonic_client(&cfg) else {
                log::warn!(
                    "remote not configured -- skipping {} downloads",
                    pending.len()
                );
                return;
            };
            for (db_id, queue_id) in pending {
                download_track(db_id, queue_id, &tx, &log_buf, &state, &cfg, &client);
            }
        })
    {
        log::error!("failed to spawn download thread: {}", e);
    }
}

#[cfg(test)]
mod share_tests {
    use super::*;
    use crate::db::queries::sample_meta;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    /// Three tracks on one album; the album carries a remote ID.
    fn album_of_three(db: &Database) -> (i64, Vec<i64>) {
        let ids: Vec<i64> = ["One", "Two", "Three"]
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let mut meta = sample_meta(title, "Boards of Canada", "Geogaddi");
                meta.path = Some(format!("/music/geogaddi/{i}.flac"));
                meta.track_number = Some(i as i32 + 1);
                queries::upsert_track(&db.conn, &meta).unwrap()
            })
            .collect();
        let album_id: i64 = db
            .conn
            .query_row("SELECT album_id FROM tracks WHERE id = ?1", [ids[0]], |r| {
                r.get(0)
            })
            .unwrap();
        db.conn
            .execute(
                "UPDATE albums SET remote_id = 'al-1' WHERE id = ?1",
                [album_id],
            )
            .unwrap();
        (album_id, ids)
    }

    fn song(id: &str, title: &str, artist: &str, secs: i64) -> crate::remote::client::SubsonicSong {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "artist": artist, "duration": secs
        }))
        .unwrap()
    }

    #[test]
    fn identifies_a_track_the_server_names_differently() {
        // The local file carries a title truncated to the 30 bytes of an ID3v1
        // tag; the server has the whole thing. Its own search bridges the gap.
        let songs = [song(
            "r1",
            "Golden Skans (David E Sugar Remix)",
            "Klaxons",
            320,
        )];
        assert_eq!(
            sole_match(&songs, "Klaxons", Some(320_026)),
            Some("r1".into())
        );
    }

    #[test]
    fn two_plausible_songs_identify_neither() {
        let songs = [
            song("r1", "Wicked Little Town", "Hedwig", 200),
            song("r2", "Wicked Little Town (reprise)", "Hedwig", 200),
        ];
        assert_eq!(sole_match(&songs, "Hedwig", Some(200_000)), None);
    }

    #[test]
    fn a_different_recording_of_the_same_name_is_not_a_match() {
        let songs = [song("r1", "Untitled", "Interpol", 340)];
        assert_eq!(sole_match(&songs, "Interpol", Some(200_000)), None);
        assert_eq!(sole_match(&songs, "Editors", Some(340_000)), None);
    }

    #[test]
    fn whole_album_collapses_to_the_album_link() {
        let db = test_db();
        let (album_id, ids) = album_of_three(&db);
        assert_eq!(
            album_remote_id(&db.conn, album_id, ids.len()),
            Some("al-1".into())
        );
    }

    #[test]
    fn part_of_an_album_does_not() {
        let db = test_db();
        let (album_id, _) = album_of_three(&db);
        // Sharing an album link for two of three tracks would hand out a track
        // the user did not pick.
        assert_eq!(album_remote_id(&db.conn, album_id, 2), None);
    }

    #[test]
    fn a_local_only_album_has_no_link_to_collapse_to() {
        let db = test_db();
        let (album_id, ids) = album_of_three(&db);
        db.conn
            .execute(
                "UPDATE albums SET remote_id = NULL WHERE id = ?1",
                [album_id],
            )
            .unwrap();
        assert_eq!(album_remote_id(&db.conn, album_id, ids.len()), None);
    }
}

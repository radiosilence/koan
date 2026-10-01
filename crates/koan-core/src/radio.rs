//! Radio mode: picks tracks from the local library by several similarity axes:
//! - ListenBrainz similar artists (no API key)
//! - MusicBrainz relationships (collaborators, band members, associated acts)
//! - Subsonic getSimilarSongs2 (when remote is configured)
//! - Genre/era matching from local metadata
//! - Acoustic similarity from stored feature vectors
//! - Play history, which favours tracks not played recently
//!
//! The seed *drifts* — recent plays are weighted more heavily than the initial track,
//! so the radio evolves through your library instead of orbiting one point.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::config::RadioConfig;
use crate::db::queries;
use crate::remote::client::SubsonicClient;
use crate::remote::listenbrainz;
use crate::remote::musicbrainz;

/// Which similarity axis led to a candidate being selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimilarityAxis {
    ListenBrainz,
    MusicBrainz,
    Subsonic,
    GenreEra,
    SameArtist,
    Random,
    Acoustic,
}

/// A candidate track with its scoring breakdown.
#[derive(Debug)]
struct Candidate {
    track_id: i64,
    path: Option<String>,
    year: Option<i32>,
    /// Similarity axes that contributed to this candidate.
    axes: HashSet<SimilarityAxis>,
    /// Base similarity score (0.0..1.0).
    base_score: f64,
}

/// Context extracted from the current queue and play history to guide radio picks.
#[derive(Debug, Default)]
pub struct RadioContext {
    /// Whether the slow, network-backed signals may run. False when the queue
    /// is about to run dry and a pick is needed this instant.
    pub allow_network: bool,
    /// Artist IDs from the seed window, with recency weight (more recent = higher).
    pub seed_artists: HashMap<i64, f64>,
    /// Paths already in the queue (to avoid duplicates).
    pub queued_paths: HashSet<String>,
    /// Track ids already in the queue. Left out of every draw, which a path
    /// cannot do for a remote track: it has none until it is downloaded.
    pub queued_ids: HashSet<i64>,
    /// Track IDs in the recent play history exclusion window.
    pub excluded_track_ids: HashSet<i64>,
    /// The currently playing track's remote_id (for Subsonic similar songs).
    pub current_remote_id: Option<String>,
    /// The currently playing track's artist name (for top songs fallback).
    pub current_artist_name: Option<String>,
    /// Genres from the seed window, lowercased.
    pub seed_genres: HashSet<String>,
    /// Average year of seed tracks (for era matching).
    pub seed_avg_year: Option<i32>,
}

impl RadioContext {
    /// Build context from queue items and play history.
    ///
    /// `queue_items`: (artist_id, path) pairs from the current queue.
    /// `seed_window`: number of recent tracks to use as seeds.
    /// `history_window`: number of recent track IDs to exclude.
    pub fn build(
        conn: &Connection,
        queue_items: &[(Option<i64>, Option<String>)],
        seed_window: usize,
        history_window: usize,
    ) -> Self {
        let mut ctx = Self {
            // Enrichment on by default; the caller turns it off when it cannot
            // wait for it.
            allow_network: true,
            ..Self::default()
        };

        // Add queued paths for duplicate prevention.
        for (_aid, path) in queue_items {
            if let Some(p) = path {
                ctx.queued_paths.insert(p.clone());
            }
        }

        // Build seed from recent plays (drifting seed).
        let recent =
            queries::recent_track_ids(conn, queries::LOCAL_USER, seed_window).unwrap_or_default();
        let seed_count = recent.len().max(1) as f64;
        let mut years: Vec<i32> = Vec::new();
        let rows: HashMap<i64, queries::TrackRow> = queries::tracks_by_ids(conn, &recent)
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        let dates = album_years(conn, &recent);

        for (i, track_id) in recent.iter().enumerate() {
            if let Some(track) = rows.get(track_id) {
                // More recent = higher weight (linear decay).
                let weight = (seed_count - i as f64) / seed_count;
                if let Some(aid) = track.artist_id {
                    let entry = ctx.seed_artists.entry(aid).or_insert(0.0);
                    *entry = entry.max(weight);
                }
                if let Some(ref genre) = track.genre {
                    ctx.seed_genres.insert(genre.to_lowercase());
                }
                if let Some(&year) = dates.get(track_id) {
                    years.push(year);
                }
            }
        }

        // With no play history, weight the artists in the queue instead.
        if ctx.seed_artists.is_empty() {
            for (artist_id, _path) in queue_items {
                if let Some(aid) = artist_id {
                    *ctx.seed_artists.entry(*aid).or_default() += 1.0;
                }
            }
            // Normalise.
            let max = ctx.seed_artists.values().copied().fold(1.0_f64, f64::max);
            for v in ctx.seed_artists.values_mut() {
                *v /= max;
            }

            // The queued artists' genres.
            let artist_ids: Vec<i64> = ctx.seed_artists.keys().copied().collect();
            ctx.seed_genres.extend(
                queries::genres_by_artist_ids(conn, &artist_ids)
                    .unwrap_or_default()
                    .into_values()
                    .flatten(),
            );
        }

        // Build exclusion window from play history.
        let excluded = queries::recent_track_ids(conn, queries::LOCAL_USER, history_window)
            .unwrap_or_default();
        ctx.excluded_track_ids = excluded.into_iter().collect();

        // Average year of the seed tracks.
        if !years.is_empty() {
            ctx.seed_avg_year = Some(years.iter().sum::<i32>() / years.len() as i32);
        }

        ctx
    }

    /// What no draw may return: the queue, and what was played recently.
    fn drawn_out(&self) -> Vec<i64> {
        self.queued_ids
            .iter()
            .chain(&self.excluded_track_ids)
            .copied()
            .collect()
    }

    /// Context from queue items alone, with no database behind it.
    #[cfg(test)]
    pub fn from_queue(items: &[(Option<i64>, Option<String>)]) -> Self {
        let mut ctx = Self::default();
        for (artist_id, path) in items {
            if let Some(aid) = artist_id {
                *ctx.seed_artists.entry(*aid).or_default() += 1.0;
            }
            if let Some(p) = path {
                ctx.queued_paths.insert(p.clone());
            }
        }
        // Normalise.
        let max = ctx.seed_artists.values().copied().fold(1.0_f64, f64::max);
        if max > 0.0 {
            for v in ctx.seed_artists.values_mut() {
                *v /= max;
            }
        }
        ctx
    }
}

/// Pick tracks for radio mode. Returns track IDs to enqueue.
///
/// Multi-signal strategy with fallback chain:
/// 1. ListenBrainz similar artists -> local tracks
/// 2. MusicBrainz relationships -> local tracks by collaborators/associated acts
/// 3. Subsonic getSimilarSongs2 (if remote configured)
/// 4. Genre + era match -> local tracks with matching tags from similar decade
/// 5. Same-artist fallback
/// 6. Acoustic similarity (vector KNN)
/// 7. Random from library, the last resort
pub fn pick_tracks(
    conn: &Connection,
    ctx: &RadioContext,
    client: Option<&SubsonicClient>,
    config: &RadioConfig,
) -> Vec<i64> {
    let count = config.batch_size;
    let mut candidates: Vec<Candidate> = Vec::new();

    log::info!(
        "radio: picking {} tracks (seed: {} artists, {} genres, {} excluded, remote_id={}, artist={})",
        count,
        ctx.seed_artists.len(),
        ctx.seed_genres.len(),
        ctx.excluded_track_ids.len(),
        ctx.current_remote_id.as_deref().unwrap_or("none"),
        ctx.current_artist_name.as_deref().unwrap_or("none"),
    );

    // Signals 1-3 go to the network, and MusicBrainz is held to one request a
    // second per seed artist, so they run only for a caller that can wait for
    // them. The local signals are a database read.
    if ctx.allow_network {
        // --- Signal 1: ListenBrainz similar artists ---
        gather_listenbrainz_candidates(conn, ctx, &mut candidates);

        // --- Signal 2: MusicBrainz relationships ---
        gather_musicbrainz_candidates(conn, ctx, &mut candidates);

        // --- Signal 3: Subsonic similar songs ---
        if let Some(client) = client {
            gather_subsonic_candidates(conn, ctx, client, &mut candidates);
        }
    }

    // --- Signal 4: Genre + era match ---
    gather_genre_era_candidates(conn, ctx, &mut candidates);

    // --- Signal 5: Same-artist tracks ---
    gather_same_artist_candidates(conn, ctx, &mut candidates);

    // --- Signal 6: Acoustic similarity (vector KNN) ---
    gather_acoustic_candidates(conn, config.seed_window, &mut candidates);

    // --- Signal 7: Random library tracks ---
    gather_random_candidates(conn, ctx, &mut candidates);

    log::info!("radio: {} raw candidates before scoring", candidates.len());

    // Deduplicate by track_id, merging axes.
    let mut deduped: HashMap<i64, Candidate> = HashMap::new();
    for c in candidates {
        let entry = deduped.entry(c.track_id).or_insert_with(|| Candidate {
            track_id: c.track_id,
            path: c.path.clone(),
            year: c.year,
            axes: HashSet::new(),
            base_score: 0.0,
        });
        entry.axes.extend(c.axes.iter());
        entry.base_score = entry.base_score.max(c.base_score);
    }

    // Filter out excluded tracks and already-queued.
    let mut scored: Vec<(i64, f64)> = deduped
        .into_values()
        .filter(|c| !ctx.excluded_track_ids.contains(&c.track_id))
        .filter(|c| {
            c.path
                .as_ref()
                .is_none_or(|p| !ctx.queued_paths.contains(p))
        })
        .map(|c| {
            let score = compute_score(conn, &c, ctx, config);
            (c.track_id, score)
        })
        .collect();

    // Sort by score descending.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Weighted random selection from top candidates for variety.
    let picks = weighted_select(&scored, count);

    log::info!("radio: picked {} tracks", picks.len());
    picks
}

/// Compute final score for a candidate.
fn compute_score(
    conn: &Connection,
    candidate: &Candidate,
    ctx: &RadioContext,
    config: &RadioConfig,
) -> f64 {
    let base = candidate.base_score;

    // Signal overlap bonus: tracks matching on 2+ axes score higher.
    let overlap_bonus = match candidate.axes.len() {
        0 | 1 => 1.0,
        2 => 1.5,
        3 => 2.0,
        _ => 2.5,
    };

    // Recency bonus: boost tracks that haven't been played recently or ever.
    let recency_bonus = compute_recency_bonus(conn, candidate.track_id, config.discovery_weight);

    // Era proximity bonus (if we have year data).
    let era_bonus = if let (Some(track_year), Some(seed_year)) = (candidate.year, ctx.seed_avg_year)
    {
        let diff = (track_year - seed_year).unsigned_abs();
        if diff <= 5 {
            1.3
        } else if diff <= 10 {
            1.1
        } else {
            1.0
        }
    } else {
        1.0
    };

    base * overlap_bonus * recency_bonus * era_bonus
}

/// Compute recency bonus for a track. Higher = more desirable.
/// Never-played tracks get the highest bonus.
fn compute_recency_bonus(conn: &Connection, track_id: i64, discovery_weight: f64) -> f64 {
    let last_played = queries::last_played_at(conn, queries::LOCAL_USER, track_id).unwrap_or(None);
    match last_played {
        None => {
            // Never played — big bonus, scaled by discovery_weight.
            1.0 + discovery_weight * 2.0
        }
        Some(ts) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let days_ago = (now - ts) / 86400;
            if days_ago > 180 {
                1.0 + discovery_weight * 1.5 // Not heard in six months.
            } else if days_ago > 30 {
                1.0 + discovery_weight * 0.8
            } else if days_ago > 7 {
                1.0 + discovery_weight * 0.3
            } else {
                1.0 // Recently played — no bonus.
            }
        }
    }
}

/// Weighted random selection from scored candidates.
/// Takes the top N*3 candidates and selects N with probability proportional to score.
fn weighted_select(scored: &[(i64, f64)], count: usize) -> Vec<i64> {
    if scored.is_empty() {
        return vec![];
    }

    let pool_size = (count * 3).min(scored.len());
    let pool = &scored[..pool_size];

    // Simple selection from the top-scored pool.
    let mut selected = Vec::new();
    let mut used = HashSet::new();

    for (id, _score) in pool {
        if selected.len() >= count {
            break;
        }
        if used.insert(*id) {
            selected.push(*id);
        }
    }

    selected
}

// --- Signal gatherers ---

fn gather_listenbrainz_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    let http = reqwest::blocking::Client::new();

    for (&artist_id, &weight) in ctx.seed_artists.iter().take(3) {
        // Get artist MBID from our DB.
        let mbid: Option<String> = conn
            .query_row(
                "SELECT mbid FROM artists WHERE id = ?1",
                rusqlite::params![artist_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let mbid = match mbid {
            Some(m) if !m.is_empty() => m,
            _ => {
                // Try to look up MBID via MusicBrainz search.
                let artist_name: Option<String> = conn
                    .query_row(
                        "SELECT name FROM artists WHERE id = ?1",
                        rusqlite::params![artist_id],
                        |row| row.get(0),
                    )
                    .ok();
                if let Some(name) = artist_name {
                    match musicbrainz::lookup_artist_mbid(&http, &name) {
                        Ok(Some(mbid)) => {
                            // Cache the MBID.
                            let _ = conn.execute(
                                "UPDATE artists SET mbid = ?1 WHERE id = ?2",
                                rusqlite::params![mbid, artist_id],
                            );
                            mbid
                        }
                        _ => continue,
                    }
                } else {
                    continue;
                }
            }
        };

        // Check if we have fresh ListenBrainz data cached.
        if queries::has_fresh_similar_artists_for_source(conn, artist_id, Some("listenbrainz"))
            .unwrap_or(false)
        {
            // Use cached data.
            add_cached_similar_candidates(
                conn,
                ctx,
                artist_id,
                weight,
                SimilarityAxis::ListenBrainz,
                candidates,
            );
            continue;
        }

        // Fetch from API.
        match listenbrainz::get_similar_artists(&http, &mbid, 20) {
            Ok(similar) => {
                log::info!(
                    "radio: listenbrainz returned {} similar for artist_id={}",
                    similar.len(),
                    artist_id
                );
                // Match to local artists and cache.
                let mut pairs: Vec<(i64, f64)> = Vec::new();
                for sa in &similar {
                    // Try to find by MBID first, then by name.
                    let local_id: Option<i64> = conn
                        .query_row(
                            "SELECT id FROM artists WHERE mbid = ?1",
                            rusqlite::params![sa.mbid],
                            |row| row.get(0),
                        )
                        .ok()
                        .or_else(|| {
                            conn.query_row(
                                "SELECT id FROM artists WHERE name = ?1 COLLATE NOCASE",
                                rusqlite::params![sa.name],
                                |row| row.get(0),
                            )
                            .ok()
                        });

                    if let Some(local_id) = local_id
                        && local_id != artist_id
                    {
                        pairs.push((local_id, sa.score));
                    }
                }

                if !pairs.is_empty() {
                    let _ = queries::save_similar_artists(conn, artist_id, &pairs, "listenbrainz");
                }

                // Add candidates from the matched local artists.
                add_local_artist_candidates(
                    conn,
                    ctx,
                    &pairs,
                    weight,
                    SimilarityAxis::ListenBrainz,
                    candidates,
                );
            }
            Err(e) => {
                log::debug!(
                    "radio: listenbrainz failed for artist_id={}: {}",
                    artist_id,
                    e
                );
                // Fall through — other signals will pick up the slack.
            }
        }
    }
}

fn gather_musicbrainz_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    let http = musicbrainz::default_client();

    for (&artist_id, &weight) in ctx.seed_artists.iter().take(3) {
        // Check cache first.
        if queries::has_fresh_similar_artists_for_source(conn, artist_id, Some("musicbrainz"))
            .unwrap_or(false)
        {
            add_cached_similar_candidates(
                conn,
                ctx,
                artist_id,
                weight,
                SimilarityAxis::MusicBrainz,
                candidates,
            );
            continue;
        }

        let mbid: Option<String> = conn
            .query_row(
                "SELECT mbid FROM artists WHERE id = ?1",
                rusqlite::params![artist_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        let Some(mbid) = mbid.filter(|m| !m.is_empty()) else {
            continue;
        };

        match musicbrainz::get_artist_relations(&http, &mbid) {
            Ok(relations) => {
                log::info!(
                    "radio: musicbrainz returned {} relations for artist_id={}",
                    relations.len(),
                    artist_id
                );

                let mut pairs: Vec<(i64, f64)> = Vec::new();

                for rel in &relations {
                    let local_id: Option<i64> = conn
                        .query_row(
                            "SELECT id FROM artists WHERE mbid = ?1",
                            rusqlite::params![rel.mbid],
                            |row| row.get(0),
                        )
                        .ok()
                        .or_else(|| {
                            conn.query_row(
                                "SELECT id FROM artists WHERE name = ?1 COLLATE NOCASE",
                                rusqlite::params![rel.name],
                                |row| row.get(0),
                            )
                            .ok()
                        });

                    if let Some(local_id) = local_id
                        && local_id != artist_id
                    {
                        // Score by relationship type.
                        let score = match rel.category {
                            musicbrainz::RelationCategory::Member => 0.8,
                            musicbrainz::RelationCategory::Collaborator => 0.7,
                            musicbrainz::RelationCategory::Associated => 0.5,
                        };
                        pairs.push((local_id, score));
                    }
                }

                if !pairs.is_empty() {
                    let _ = queries::save_similar_artists_with_rel(
                        conn,
                        artist_id,
                        &pairs,
                        "musicbrainz",
                        "collaborator",
                    );
                }

                add_local_artist_candidates(
                    conn,
                    ctx,
                    &pairs,
                    weight,
                    SimilarityAxis::MusicBrainz,
                    candidates,
                );
            }
            Err(e) => {
                log::debug!(
                    "radio: musicbrainz relations failed for artist_id={}: {}",
                    artist_id,
                    e
                );
            }
        }

        // Rate limit: sleep 1s between MusicBrainz requests.
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn gather_subsonic_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    client: &SubsonicClient,
    candidates: &mut Vec<Candidate>,
) {
    if let Some(ref remote_id) = ctx.current_remote_id {
        match client.get_similar_songs(remote_id, 30) {
            Ok(songs) => {
                log::info!("radio: subsonic returned {} similar songs", songs.len());
                for (i, song) in songs.iter().enumerate() {
                    if let Some(track_id) = resolve_subsonic_song_to_track(conn, song) {
                        let score = (songs.len() as f64 - i as f64) / songs.len() as f64;
                        let track = queries::get_track_row(conn, track_id).ok().flatten();
                        candidates.push(Candidate {
                            track_id,
                            path: track.as_ref().and_then(|t| t.path.clone()),
                            year: None,
                            axes: [SimilarityAxis::Subsonic].into_iter().collect(),
                            base_score: score * 0.9,
                        });
                    }
                }

                // Cache artist relationships from subsonic results.
                cache_subsonic_artist_relationships(conn, ctx, &songs);
            }
            Err(e) => {
                log::debug!("radio: subsonic similar songs failed: {}", e);
            }
        }
    }
}

fn gather_genre_era_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    if ctx.seed_genres.is_empty() {
        return;
    }

    // Drawn by genre, ten from each of three of them, so every candidate here
    // has one of the seed's genres.
    let exclude = ctx.drawn_out();
    let mut tracks = Vec::new();
    for genre in ctx.seed_genres.iter().take(3) {
        let filter = queries::RandomFilter {
            genre: Some(genre),
            exclude: &exclude,
            ..Default::default()
        };
        match queries::random_tracks_where(conn, 10, &filter) {
            Ok(drawn) => tracks.extend(drawn),
            Err(e) => log::debug!("radio: genre/era query failed: {}", e),
        }
    }
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let years = album_years(conn, &ids);

    for track in tracks {
        let year = years.get(&track.id).copied();

        // Score higher if both genre AND era match.
        let genre_match = track
            .genre
            .as_ref()
            .is_some_and(|g| ctx.seed_genres.contains(&g.to_lowercase()));
        let era_match = match (year, ctx.seed_avg_year) {
            (Some(y), Some(sy)) => (y as i64 - sy as i64).unsigned_abs() <= 10,
            _ => false,
        };

        let base_score = match (genre_match, era_match) {
            (true, true) => 0.6,
            (true, false) => 0.3,
            (false, true) => 0.2,
            (false, false) => 0.1,
        };

        candidates.push(Candidate {
            track_id: track.id,
            path: track.path.clone(),
            year,
            axes: [SimilarityAxis::GenreEra].into_iter().collect(),
            base_score,
        });
    }
}

fn gather_same_artist_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    // The five most heavily weighted seed artists, three tracks each, drawn
    // through the artist index.
    let mut artists: Vec<(i64, f64)> = ctx.seed_artists.iter().map(|(&a, &w)| (a, w)).collect();
    artists.sort_by(|a, b| b.1.total_cmp(&a.1));
    let exclude = ctx.drawn_out();
    for (artist_id, weight) in artists.into_iter().take(5) {
        let filter = queries::RandomFilter {
            artist_id: Some(artist_id),
            exclude: &exclude,
            ..Default::default()
        };
        match queries::random_tracks_where(conn, 3, &filter) {
            Ok(tracks) => {
                for track in tracks {
                    candidates.push(Candidate {
                        track_id: track.id,
                        path: track.path.clone(),
                        year: None,
                        axes: [SimilarityAxis::SameArtist].into_iter().collect(),
                        base_score: weight * 0.4, // Lower base — same-artist is the fallback.
                    });
                }
            }
            Err(e) => {
                log::debug!("radio: same-artist query failed: {}", e);
            }
        }
    }
}

fn gather_acoustic_candidates(
    conn: &Connection,
    seed_window: usize,
    candidates: &mut Vec<Candidate>,
) {
    // The same seeds that drive seed_artists — one window, one answer.
    let seed_ids =
        queries::recent_track_ids(conn, queries::LOCAL_USER, seed_window).unwrap_or_default();
    let mut seed_embeddings = Vec::new();
    for tid in &seed_ids {
        if let Ok(Some(emb)) = queries::get_vector(conn, *tid) {
            seed_embeddings.push(emb);
        }
    }

    if seed_embeddings.is_empty() {
        return;
    }

    let centroid = crate::index::features::centroid(&seed_embeddings);
    let knn_result = queries::find_similar_to_vector(conn, &centroid, 30, None);
    match knn_result {
        Ok(ref results) => {
            let max_dist = results.last().map(|r| r.1).unwrap_or(1.0).max(0.001);
            let mut added = 0;
            for &(track_id, dist) in results {
                // Skip seed tracks themselves.
                if seed_ids.contains(&track_id) {
                    continue;
                }
                // Score: inverse of normalised distance. Closer = higher score.
                let score = (1.0 - (dist / max_dist)).max(0.0) as f64 * 0.7;
                let track = queries::get_track_row(conn, track_id).ok().flatten();
                candidates.push(Candidate {
                    track_id,
                    path: track.as_ref().and_then(|t| t.path.clone()),
                    year: None,
                    axes: [SimilarityAxis::Acoustic].into_iter().collect(),
                    base_score: score,
                });
                added += 1;
            }
            log::info!(
                "radio: acoustic signal added {} candidates from {} seed vectors",
                added,
                seed_embeddings.len()
            );
        }
        Err(e) => {
            log::debug!("radio: acoustic similarity query failed: {}", e);
        }
    }
}

fn gather_random_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    candidates: &mut Vec<Candidate>,
) {
    let exclude = ctx.drawn_out();
    let filter = queries::RandomFilter {
        exclude: &exclude,
        ..Default::default()
    };
    match queries::random_tracks_where(conn, 10, &filter) {
        Ok(tracks) => {
            for track in tracks {
                candidates.push(Candidate {
                    track_id: track.id,
                    path: track.path.clone(),
                    year: None,
                    axes: [SimilarityAxis::Random].into_iter().collect(),
                    base_score: 0.05, // Last resort: any track beats silence.
                });
            }
        }
        Err(e) => {
            log::debug!("radio: random fallback failed: {}", e);
        }
    }
}

// --- Helpers ---

/// The release year of each track's album, for the tracks that have one.
fn album_years(conn: &Connection, track_ids: &[i64]) -> HashMap<i64, i32> {
    queries::queue_item_extras(conn, track_ids)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, e)| {
            let year = crate::helpers::year_of(e.album_date.as_deref()?)?;
            Some((id, year.parse().ok()?))
        })
        .collect()
}

/// Add candidates from cached similar artist data.
fn add_cached_similar_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    artist_id: i64,
    seed_weight: f64,
    axis: SimilarityAxis,
    candidates: &mut Vec<Candidate>,
) {
    if let Ok(similar) = queries::get_similar_artists(conn, artist_id) {
        let pairs: Vec<(i64, f64)> = similar.into_iter().map(|(a, s)| (a.id, s)).collect();
        add_local_artist_candidates(conn, ctx, &pairs, seed_weight, axis, candidates);
    }
}

/// Add candidates from a list of (artist_id, similarity_score) pairs.
fn add_local_artist_candidates(
    conn: &Connection,
    ctx: &RadioContext,
    pairs: &[(i64, f64)],
    seed_weight: f64,
    axis: SimilarityAxis,
    candidates: &mut Vec<Candidate>,
) {
    let exclude = ctx.drawn_out();
    for &(similar_artist_id, sim_score) in pairs.iter().take(10) {
        let filter = queries::RandomFilter {
            artist_id: Some(similar_artist_id),
            exclude: &exclude,
            ..Default::default()
        };
        if let Ok(tracks) = queries::random_tracks_where(conn, 3, &filter) {
            for track in tracks {
                candidates.push(Candidate {
                    track_id: track.id,
                    path: track.path.clone(),
                    year: None,
                    axes: [axis].into_iter().collect(),
                    base_score: sim_score * seed_weight * 0.8,
                });
            }
        }
    }
}

/// Resolve a SubsonicSong to a local track ID by remote_id.
fn resolve_subsonic_song_to_track(
    conn: &Connection,
    song: &crate::remote::client::SubsonicSong,
) -> Option<i64> {
    conn.query_row(
        "SELECT id FROM tracks WHERE remote_id = ?1",
        rusqlite::params![song.id],
        |row| row.get::<_, i64>(0),
    )
    .ok()
}

/// Extract and cache artist relationships from Subsonic similar songs response.
fn cache_subsonic_artist_relationships(
    conn: &Connection,
    ctx: &RadioContext,
    songs: &[crate::remote::client::SubsonicSong],
) {
    for &artist_id in ctx.seed_artists.keys().take(5) {
        let mut similar: HashMap<i64, f64> = HashMap::new();
        let total = songs.len() as f64;

        for (i, song) in songs.iter().enumerate() {
            if let Some(ref song_artist_id) = song.artist_id {
                let local_artist_id: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM artists WHERE remote_id = ?1",
                        rusqlite::params![song_artist_id],
                        |row| row.get(0),
                    )
                    .ok();

                if let Some(local_id) = local_artist_id
                    && local_id != artist_id
                {
                    let score = (total - i as f64) / total;
                    let entry = similar.entry(local_id).or_insert(0.0);
                    *entry = entry.max(score);
                }
            }
        }

        if !similar.is_empty() {
            let pairs: Vec<(i64, f64)> = similar.into_iter().collect();
            let _ = queries::save_similar_artists(conn, artist_id, &pairs, "subsonic");
        }
    }
}

// ---------------------------------------------------------------------------
// Auto-queue
// ---------------------------------------------------------------------------

/// Keep the queue topped up while radio mode is on.
///
/// Radio mode is a flag on `SharedPlayerState`. Owning the loop here means
/// every front end that sets it gets the same behaviour instead of
/// reimplementing it.
///
/// Runs on its own thread and exits when the player goes away.
pub fn spawn_autoqueue(
    state: std::sync::Arc<crate::player::state::SharedPlayerState>,
    tx: crossbeam_channel::Sender<crate::player::commands::PlayerCommand>,
) {
    use crate::player::commands::PlayerCommand;
    use crate::player::state::{QueueEntryStatus, QueueItemId};

    std::thread::Builder::new()
        .name("koan-radio".into())
        .spawn(move || {
            use std::time::{Duration, Instant};

            // Read when radio is switched on, and again a minute later if it
            // is still on: settings changed while it plays still apply, and a
            // two-second loop does not parse two files every pass.
            let mut cfg: Option<(crate::config::Config, Instant)> = None;
            // A top-up that found nothing, and the queue it found nothing for.
            // Nothing is tried again until the queue moves or a minute passes,
            // either of which may give the picker something new to go on.
            let mut fruitless: Option<(u64, Instant)> = None;
            loop {
                std::thread::sleep(Duration::from_secs(2));

                if !state.radio_mode() || state.cursor().is_none() {
                    cfg = None;
                    continue;
                }
                log::debug!("radio: awake, cursor set");

                if fruitless.is_some_and(|(version, at)| {
                    version == state.playlist_version() && at.elapsed() < Duration::from_secs(60)
                }) {
                    continue;
                }

                if cfg
                    .as_ref()
                    .is_none_or(|(_, read)| read.elapsed() > Duration::from_secs(60))
                {
                    cfg = Some((
                        crate::config::Config::load().unwrap_or_default(),
                        Instant::now(),
                    ));
                }
                let Some((cfg, _)) = &cfg else { continue };
                let snapshot = state.derive_visible_queue();
                let Some(playing) = snapshot
                    .entries
                    .iter()
                    .position(|e| e.status == QueueEntryStatus::Playing)
                else {
                    log::debug!("radio: nothing is playing, waiting");
                    continue;
                };
                let remaining = snapshot
                    .entries
                    .iter()
                    .skip(playing + 1)
                    .filter(|e| e.status == QueueEntryStatus::Queued)
                    .count();
                if remaining > cfg.radio.lookahead {
                    continue;
                }
                log::info!(
                    "radio: {} queued after the cursor, topping up to {}",
                    remaining,
                    cfg.radio.lookahead
                );

                let Ok(db) = crate::db::pool::shared().get() else {
                    continue;
                };
                let version = state.playlist_version();
                let (items, cursor) = state.snapshot_playlist();

                // Each item's row, read once for the whole queue. An item
                // without a database id was played from a file, and is looked
                // up by its path.
                let item_ids: Vec<Option<i64>> = items
                    .iter()
                    .map(|item| {
                        item.db_id.or_else(|| {
                            let path = item.path.to_str()?;
                            queries::track_id_by_path(&db.conn, path).ok().flatten()
                        })
                    })
                    .collect();
                let ids: Vec<i64> = item_ids.iter().flatten().copied().collect();
                let queue_rows: HashMap<i64, queries::TrackRow> =
                    queries::tracks_by_ids(&db.conn, &ids)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|t| (t.id, t))
                        .collect();
                let row_of = |i: usize| item_ids[i].and_then(|id| queue_rows.get(&id));

                // The seed drifts: recent items weigh more than the first thing
                // queued, so the radio moves through the library rather than
                // orbiting one track.
                let context: Vec<(Option<i64>, Option<String>)> = items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        (
                            row_of(i).and_then(|t| t.artist_id),
                            Some(item.path.to_string_lossy().into_owned()),
                        )
                    })
                    .collect();

                let mut ctx = RadioContext::build(
                    &db.conn,
                    &context,
                    cfg.radio.seed_window,
                    cfg.radio.history_window,
                );
                ctx.queued_ids = queue_rows.keys().copied().collect();
                if let Some(current) = cursor.and_then(|cid| items.iter().position(|i| i.id == cid))
                    && let Some(row) = row_of(current)
                {
                    ctx.current_remote_id = row.remote_id.clone();
                    ctx.current_artist_name = Some(row.artist_name.clone());
                }
                // Local signals only.
                //
                // ListenBrainz and MusicBrainz each rate-limit to one request a
                // second per seed artist, and both are called in line before a
                // single pick comes back — so a queue that needs a track in the
                // next few seconds gets one long after the music has stopped.
                // Genre/era, same-artist, acoustic similarity and plain random
                // are all database reads and answer immediately, which is worth
                // more than a better-chosen track that arrives too late.

                ctx.allow_network = false;

                // No client, and no similar-artist prefetch: both are HTTP
                // round trips in front of a pick that is needed now. Cached
                // similar artists are still read from the database by the local
                // signals; nothing is fetched.
                let picks = pick_tracks(&db.conn, &ctx, None, &cfg.radio);
                if picks.is_empty() {
                    log::warn!("radio: the picker returned nothing for this seed");
                    fruitless = Some((version, Instant::now()));
                    continue;
                }

                // Never queue something already in the queue: the picker scores
                // by similarity and has no idea what is sitting below the
                // cursor.
                let queued: HashSet<String> = items
                    .iter()
                    .map(|i| i.path.to_string_lossy().into_owned())
                    .collect();
                let rows: Vec<_> = queries::tracks_by_ids(&db.conn, &picks)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|row| {
                        row.path
                            .as_deref()
                            .or(row.cached_path.as_deref())
                            .is_none_or(|p| !queued.contains(p))
                    })
                    .collect();
                if rows.is_empty() {
                    log::warn!("radio: every pick was already in the queue");
                    fruitless = Some((version, Instant::now()));
                    continue;
                }
                fruitless = None;

                let new_items = crate::helpers::playlist_items_for_tracks(&db, &rows);
                let pending: Vec<(i64, QueueItemId)> = new_items
                    .iter()
                    .filter(|i| matches!(i.state, crate::player::state::ItemState::Pending))
                    .filter_map(|i| i.db_id.map(|id| (id, i.id)))
                    .collect();

                log::info!("radio: queueing {} tracks", new_items.len());
                if tx.send(PlayerCommand::AddToPlaylist(new_items)).is_err() {
                    return; // Player gone; so is the app.
                }
                if !pending.is_empty() {
                    crate::helpers::spawn_downloads(pending, tx.clone(), state.clone());
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{get_or_create_artist, sample_meta, upsert_track};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn test_radio_context_from_queue() {
        let ctx = RadioContext::from_queue(&[
            (Some(1), Some("/a.flac".into())),
            (Some(2), Some("/b.flac".into())),
            (Some(1), Some("/c.flac".into())),
        ]);
        assert_eq!(ctx.seed_artists.len(), 2);
        assert!(ctx.seed_artists[&1] > ctx.seed_artists[&2]);
        assert_eq!(ctx.queued_paths.len(), 3);
    }

    #[test]
    fn test_recency_bonus_never_played() {
        let db = test_db();
        let mut meta = sample_meta("T1", "A1", "Al1");
        meta.path = Some("/music/T1.flac".into());
        upsert_track(&db.conn, &meta).unwrap();

        let track_id: i64 = db
            .conn
            .query_row("SELECT id FROM tracks LIMIT 1", [], |row| row.get(0))
            .unwrap();

        let bonus = compute_recency_bonus(&db.conn, track_id, 0.3);
        assert!(bonus > 1.0, "never-played should get a bonus");
    }

    #[test]
    fn test_recency_bonus_recently_played() {
        let db = test_db();
        let mut meta = sample_meta("T1", "A1", "Al1");
        meta.path = Some("/music/T1.flac".into());
        upsert_track(&db.conn, &meta).unwrap();

        let track_id: i64 = db
            .conn
            .query_row("SELECT id FROM tracks LIMIT 1", [], |row| row.get(0))
            .unwrap();

        queries::record_play(
            &db.conn,
            crate::db::queries::LOCAL_USER,
            track_id,
            Some(240_000),
        )
        .unwrap();

        let bonus = compute_recency_bonus(&db.conn, track_id, 0.3);
        assert!(
            (bonus - 1.0).abs() < f64::EPSILON,
            "recently played should get no bonus"
        );
    }

    #[test]
    fn test_weighted_select_empty() {
        assert!(weighted_select(&[], 5).is_empty());
    }

    #[test]
    fn test_weighted_select_fewer_than_requested() {
        let scored = vec![(1, 0.9), (2, 0.5)];
        let picks = weighted_select(&scored, 5);
        assert_eq!(picks.len(), 2);
    }

    #[test]
    fn test_signal_overlap_scoring() {
        let db = test_db();
        let config = RadioConfig::default();
        let ctx = RadioContext::default();

        // Candidate with 1 axis.
        let c1 = Candidate {
            track_id: 1,
            path: None,
            year: None,
            axes: [SimilarityAxis::ListenBrainz].into_iter().collect(),
            base_score: 0.5,
        };

        // Candidate with 3 axes.
        let c3 = Candidate {
            track_id: 2,
            path: None,
            year: None,
            axes: [
                SimilarityAxis::ListenBrainz,
                SimilarityAxis::MusicBrainz,
                SimilarityAxis::GenreEra,
            ]
            .into_iter()
            .collect(),
            base_score: 0.5,
        };

        let score1 = compute_score(&db.conn, &c1, &ctx, &config);
        let score3 = compute_score(&db.conn, &c3, &ctx, &config);

        assert!(
            score3 > score1,
            "multi-axis candidate should score higher: {} vs {}",
            score3,
            score1
        );
    }

    #[test]
    fn test_pick_tracks_empty_library() {
        let db = test_db();
        let ctx = RadioContext::from_queue(&[]);
        let config = RadioConfig::default();
        let picks = pick_tracks(&db.conn, &ctx, None, &config);
        assert!(picks.is_empty());
    }

    #[test]
    fn test_pick_tracks_with_library() {
        let db = test_db();

        // Populate library.
        for i in 0..20 {
            let mut meta = sample_meta(
                &format!("Track{}", i),
                &format!("Artist{}", i % 5),
                &format!("Album{}", i % 3),
            );
            meta.path = Some(format!("/music/Album{}/Track{}.flac", i % 3, i));
            meta.track_number = Some(i);
            upsert_track(&db.conn, &meta).unwrap();
        }

        let artist_id: i64 = db
            .conn
            .query_row("SELECT id FROM artists LIMIT 1", [], |row| row.get(0))
            .unwrap();

        let ctx = RadioContext::from_queue(&[(Some(artist_id), Some("/queued.flac".into()))]);
        let config = RadioConfig {
            batch_size: 5,
            ..RadioConfig::default()
        };

        let picks = pick_tracks(&db.conn, &ctx, None, &config);
        assert!(
            !picks.is_empty(),
            "should pick at least some tracks from a populated library"
        );
        assert!(picks.len() <= 5);
    }

    #[test]
    fn test_pick_tracks_excludes_history() {
        let db = test_db();

        // Insert a few tracks.
        for i in 0..5 {
            let mut meta = sample_meta(&format!("T{}", i), "Artist", "Album");
            meta.path = Some(format!("/music/T{}.flac", i));
            meta.track_number = Some(i);
            upsert_track(&db.conn, &meta).unwrap();
        }

        // Record all as recently played.
        let mut ids = Vec::new();
        for i in 0..5 {
            let id: i64 = db
                .conn
                .query_row(
                    "SELECT id FROM tracks WHERE path = ?1",
                    rusqlite::params![format!("/music/T{}.flac", i)],
                    |row| row.get(0),
                )
                .unwrap();
            queries::record_play(&db.conn, crate::db::queries::LOCAL_USER, id, Some(240_000))
                .unwrap();
            ids.push(id);
        }

        let mut ctx = RadioContext::from_queue(&[]);
        ctx.excluded_track_ids = ids.into_iter().collect();

        let config = RadioConfig {
            batch_size: 5,
            ..RadioConfig::default()
        };

        let picks = pick_tracks(&db.conn, &ctx, None, &config);
        // All tracks are excluded, so nothing should be picked.
        assert!(
            picks.is_empty(),
            "all tracks in exclusion window, got picks"
        );
    }

    #[test]
    fn test_radio_context_build_with_no_history() {
        let db = test_db();

        for i in 0..5 {
            let _id = get_or_create_artist(&db.conn, &format!("Artist{}", i), None).unwrap();
        }

        let queue = vec![
            (Some(1_i64), Some("/a.flac".to_string())),
            (Some(2), Some("/b.flac".to_string())),
        ];
        let ctx = RadioContext::build(&db.conn, &queue, 5, 200);

        // Should fall back to queue weights since no play history.
        assert_eq!(ctx.seed_artists.len(), 2);
        assert_eq!(ctx.queued_paths.len(), 2);
    }

    /// Three tracks by one artist among three hundred by others.
    fn library_with_seed_artist(db: &Database) -> i64 {
        for i in 0..300 {
            let mut meta = sample_meta(&format!("Other{i}"), &format!("Other{}", i % 30), "Mix");
            meta.path = Some(format!("/music/other/{i}.flac"));
            meta.genre = Some("Pop".into());
            upsert_track(&db.conn, &meta).unwrap();
        }
        for i in 0..3 {
            let mut meta = sample_meta(&format!("Seed{i}"), "Seed", "Seeds");
            meta.path = Some(format!("/music/seed/{i}.flac"));
            meta.genre = Some("IDM".into());
            upsert_track(&db.conn, &meta).unwrap();
        }
        get_or_create_artist(&db.conn, "Seed", None).unwrap()
    }

    #[test]
    fn same_artist_picks_are_by_that_artist() {
        let db = test_db();
        let seed = library_with_seed_artist(&db);
        let ctx = RadioContext::from_queue(&[(Some(seed), None)]);

        let mut candidates = Vec::new();
        gather_same_artist_candidates(&db.conn, &ctx, &mut candidates);

        let ids: Vec<i64> = candidates.iter().map(|c| c.track_id).collect();
        let rows = queries::tracks_by_ids(&db.conn, &ids).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|t| t.artist_id == Some(seed)));
    }

    #[test]
    fn genre_picks_have_the_seed_genre() {
        let db = test_db();
        let seed = library_with_seed_artist(&db);
        let ctx = RadioContext::build(&db.conn, &[(Some(seed), None)], 5, 200);
        assert_eq!(ctx.seed_genres, HashSet::from(["idm".to_string()]));

        let mut candidates = Vec::new();
        gather_genre_era_candidates(&db.conn, &ctx, &mut candidates);

        let ids: Vec<i64> = candidates.iter().map(|c| c.track_id).collect();
        let rows = queries::tracks_by_ids(&db.conn, &ids).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|t| t.genre.as_deref() == Some("IDM")));
    }

    #[test]
    fn queued_tracks_are_never_picked() {
        let db = test_db();
        let seed = library_with_seed_artist(&db);
        let mut ctx = RadioContext::from_queue(&[(Some(seed), None)]);
        ctx.queued_ids = db
            .conn
            .prepare("SELECT id FROM tracks")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        let picks = pick_tracks(&db.conn, &ctx, None, &RadioConfig::default());
        assert!(picks.is_empty());
    }
}

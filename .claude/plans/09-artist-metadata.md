# Plan 09: Artist Metadata & Discovery

Consolidated from `artist-info.md` and `07-non-tag-metadata.md`. Lyrics (Phase 1 of plan 07) are already implemented and shipped.

## Summary

Enrich koan with artist-level metadata from MusicBrainz: bio, genres, members, active years, external links. Then build on that foundation with similar artists (Last.fm), external album art (Cover Art Archive), and eventually radio mode.

## What's Already Done

- **Lyrics**: LRCLIB client, lyrics_cache table, synced/plain lyrics panel (`L` key) — all shipped in 0.4.0
- **artists.mbid column**: Exists in schema, unpopulated — ready for MusicBrainz IDs
- **reqwest**: Already a dep in koan-core (blocking client)
- **Two-layer config**: config.toml + config.local.toml — good for API keys

## Phase 1: MusicBrainz Artist Info (Medium effort, high impact)

The core feature — press `A` to see artist info. Follows the exact same pattern as lyrics.

### 1a. MusicBrainz API Client

**File:** `koan-core/src/remote/musicbrainz.rs`

Following the `lrclib.rs` pattern:
- Blocking reqwest client
- `User-Agent: koan-music/{version} (https://github.com/radiosilence/koan)`
- Rate limiting: 1 req/sec (track `Instant` of last request, sleep if needed)
- Two endpoints:
  - Search: `GET /ws/2/artist/?query=artist:{name}&fmt=json&limit=5`
  - Detail: `GET /ws/2/artist/{mbid}?inc=url-rels+genres+tags&fmt=json`

```rust
pub struct MusicBrainzArtist {
    pub mbid: String,
    pub name: String,
    pub sort_name: Option<String>,
    pub artist_type: Option<String>,     // Person, Group, Orchestra
    pub country: Option<String>,          // ISO 3166-1
    pub disambiguation: Option<String>,
    pub begin_year: Option<i32>,
    pub end_year: Option<i32>,            // None if active
    pub genres: Vec<String>,
    pub urls: Vec<(String, String)>,      // (relation_type, url)
}

pub fn search_artist(name: &str) -> Result<Vec<MusicBrainzArtist>>
pub fn get_artist(mbid: &str) -> Result<MusicBrainzArtist>
```

**No new crate deps** — reqwest + serde already available.

### 1b. DB Cache

**Table** (additive, `CREATE TABLE IF NOT EXISTS`):
```sql
CREATE TABLE IF NOT EXISTS artist_info_cache (
    id              INTEGER PRIMARY KEY,
    artist_name     TEXT NOT NULL,
    mbid            TEXT,
    artist_type     TEXT,
    country         TEXT,
    disambiguation  TEXT,
    begin_year      INTEGER,
    end_year        INTEGER,
    genres          TEXT,       -- JSON array
    urls            TEXT,       -- JSON array of [type, url] pairs
    fetched_at      INTEGER NOT NULL,
    UNIQUE(artist_name)
);
```

Cache keyed on `artist_name` (not track_id) since multiple tracks share an artist. `ON CONFLICT DO UPDATE` like lyrics. Also update `artists.mbid` when we learn an MBID.

**Query functions** (following `db/queries/lyrics.rs` pattern):
```rust
pub fn get_cached_artist_info(conn, artist_name) -> Result<Option<ArtistInfo>>
pub fn cache_artist_info(conn, info: &ArtistInfo) -> Result<()>
```

### 1c. Fetch Pipeline

**File:** `koan-core/src/artist_info.rs`

```rust
pub fn fetch_artist_info(conn: &Connection, artist_name: &str) -> Result<ArtistInfo, ArtistInfoError>
```

Pipeline:
1. Check `artist_info_cache` → return on hit
2. Search MusicBrainz by name
3. Pick best match (exact name match preferred)
4. Fetch full details with genres + URLs
5. Cache in SQLite
6. Update `artists.mbid` if matching row exists
7. Return result

Cache "not found" as tombstone (empty fields + flag) to avoid re-fetching.

### 1d. TUI Integration

Following the lyrics panel pattern exactly:

- `koan-music/src/tui/artist_info.rs`: `ArtistInfoState` + `ArtistInfoPanel` widget
- `A` key toggles bottom pane
- Spinner while fetching
- Artist change detection (like lyrics' track path detection)
- Background thread `"koan-artist-info"` with `crossbeam_channel::bounded(1)`
- Shows: name (bold), type + country + years, genres, URLs
- MusicBrainz attribution line (required by their terms)

**Layout**: When both lyrics AND artist info are active, artist info replaces lyrics position (they share the right pane). Toggle between them.

### Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `koan-core/src/remote/musicbrainz.rs` | Create | MB API client |
| `koan-core/src/remote/mod.rs` | Modify | Add `pub mod musicbrainz;` |
| `koan-core/src/artist_info.rs` | Create | Fetch pipeline + types |
| `koan-core/src/lib.rs` | Modify | Add `pub mod artist_info;` |
| `koan-core/src/db/schema.rs` | Modify | Add `artist_info_cache` table |
| `koan-core/src/db/queries/artist_info.rs` | Create | Cache get/set |
| `koan-core/src/db/queries/mod.rs` | Modify | Add module |
| `koan-music/src/tui/artist_info.rs` | Create | State + panel widget |
| `koan-music/src/tui/mod.rs` | Modify | Add module |
| `koan-music/src/tui/app.rs` | Modify | State, keybinding, tick handler |
| `koan-music/src/tui/ui.rs` | Modify | Render artist info pane |
| `koan-music/src/tui/keys.rs` | Modify | Add `A` to hint bar |

**Effort:** ~2-3 days

---

## Phase 2: Similar Artists + External Art (Medium effort, medium impact)

Builds on Phase 1's MBID infrastructure.

### 2a. Last.fm Client

**File:** `koan-core/src/remote/lastfm.rs`

- API key in config.toml (not a secret — it's an app identifier)
- Rate limit: 5 req/sec
- Endpoints: `artist.getSimilar`, `artist.getInfo`, `album.getInfo`
- For Subsonic users: `getArtistInfo2` returns `<similarArtist>` — use that first

### 2b. Similar Artists Table

```sql
CREATE TABLE IF NOT EXISTS similar_artists (
    id              INTEGER PRIMARY KEY,
    artist_id       INTEGER REFERENCES artists(id),
    similar_artist  TEXT NOT NULL,
    similar_mbid    TEXT,
    score           REAL,
    source          TEXT NOT NULL,
    fetched_at      INTEGER NOT NULL,
    UNIQUE(artist_id, similar_artist)
);
```

Display in artist info panel: "Similar artists in your library: X, Y, Z" by cross-referencing with local `artists` table.

### 2c. Cover Art Archive

Simple GET by MusicBrainz release MBID — for albums without embedded art. Cache URLs in:
```sql
CREATE TABLE IF NOT EXISTS album_art_cache (
    id          INTEGER PRIMARY KEY,
    album_id    INTEGER REFERENCES albums(id),
    image_url   TEXT NOT NULL,
    thumb_url   TEXT,
    source      TEXT NOT NULL,
    fetched_at  INTEGER NOT NULL,
    UNIQUE(album_id, source)
);
```

### 2d. Config

```toml
[metadata]
enabled = false                    # opt-in
lastfm_api_key = ""               # free, get at last.fm/api
features = ["artist_info", "similar_artists", "album_art"]
```

**Effort:** ~3-4 days

---

## Phase 3: Radio Mode (High effort, high impact)

Auto-enqueue similar tracks when the queue runs low.

### Prerequisites
- Phase 2 (similar artists data)
- Play history table

### Play History Table

```sql
CREATE TABLE IF NOT EXISTS play_history (
    id          INTEGER PRIMARY KEY,
    track_id    INTEGER REFERENCES tracks(id),
    played_at   INTEGER NOT NULL,
    duration_ms INTEGER,
    source      TEXT DEFAULT 'local'
);
```

Record play when >50% listened (standard scrobble threshold).

### Radio Mode State

```rust
struct RadioMode {
    enabled: bool,
    seed: RadioSeed,
    history: VecDeque<i64>,   // avoid repeats
}

enum RadioSeed {
    Artist(i64),
    Track(i64),
    Genre(String),
    Queue,
}
```

Track selection: similar artists (weighted) → same genre → random fallback.
For Subsonic: use `getSimilarSongs2` directly.
Auto-refill when < 3 unplayed tracks remain.

**Effort:** ~4-5 days

---

## Phase 4: Polish (Lower priority)

- ListenBrainz integration (collaborative filtering, scrobbling)
- Fanart.tv HD artist images
- Wikipedia links from MusicBrainz relationships
- Cache TTL enforcement (30-day refresh for similar artists/bios)

**Effort:** ~3-4 days

---

## Legal Notes

- **MusicBrainz**: CC0 data, free, 1 req/sec, requires User-Agent with contact info, attribution encouraged
- **Last.fm**: Non-commercial only (koan is MIT open-source, fine), attribution required ("data from Last.fm"), API key free
- **Cover Art Archive**: Free, per-image licenses (mostly permissive), keyed by MB release MBID
- **LRCLIB**: Already integrated, no terms, no key needed

Ship with no API keys. User provides Last.fm key if they want Phase 2+ features. Phase 1 (MusicBrainz) needs no key at all.

---

## Supersedes

This plan replaces:
- `.claude/plans/artist-info.md` (merged as Phase 1)
- `.claude/plans/07-non-tag-metadata.md` (lyrics done; remaining phases merged here)

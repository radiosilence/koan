# Artist Info from MusicBrainz

## Context

koan already fetches external data for tracks (lyrics from LRCLIB, cached in SQLite). This plan adds artist-level metadata from the MusicBrainz API: biography, genres, members, active years, and external links. The implementation follows the same patterns established by the lyrics feature.

### Existing patterns to follow

- **Lyrics fetch pipeline** (`koan-core/src/lyrics.rs`): DB cache check -> API call -> cache result -> return. Single `fetch_lyrics()` entry point.
- **LRCLIB client** (`koan-core/src/remote/lrclib.rs`): Blocking `reqwest` client with `USER_AGENT`, serde deserialization, custom error enum.
- **Lyrics DB cache** (`koan-core/src/db/queries/lyrics.rs`): `get_cached_*` / `cache_*` function pair with `ON CONFLICT DO UPDATE`.
- **Lyrics TUI integration** (`koan-music/src/tui/app.rs:440-535`): Track change detection in `handle_tick()`, spawn named background thread (`koan-lyrics`), `crossbeam_channel::bounded(1)` for result delivery, `try_recv()` on next tick.
- **Lyrics panel** (`koan-music/src/tui/lyrics.rs`): `LyricsState` (result + fetching flag + track path for change detection), `LyricsPanel` widget, spinner while fetching.
- **Artists table** (`koan-core/src/db/schema.rs`): Already has `mbid TEXT` column -- unused today, perfect for storing MusicBrainz artist IDs.

### Key decisions

**Display approach: Option B -- new `A` hotkey opening a bottom pane** (recommended).

Rationale:
- The track info modal (`i`) is a centered popup showing per-track metadata with cover art. Artist info is conceptually different (artist-level, text-heavy with bio).
- The lyrics panel (`L`) already established the pattern of a toggleable side/bottom pane. Artist info fits the same paradigm.
- A bottom pane can persist while browsing tracks, updating when the artist changes -- just like lyrics updates when the track changes.
- The modal approach would make track info cluttered and the two concerns (track metadata vs artist biography) would fight for space.

**API: MusicBrainz only** (no Discogs for v1).

Rationale:
- MusicBrainz is free, no API key needed, has comprehensive artist data.
- Discogs requires OAuth registration and has stricter rate limits.
- MusicBrainz alone covers: biography (via linked Wikipedia/Wikidata), genres/tags, members, type, country, active years, external URLs.
- Discogs can be added later as an enrichment source if needed.

**Rate limiting: 1 req/sec** per MusicBrainz policy.

The app makes at most 2 requests per artist lookup (search + detail), and results are cached, so rate limiting is simple -- just a `std::thread::sleep` guard or a timestamp check. No need for a token bucket.

## Work Objectives

Add an artist info feature that:
1. Fetches artist metadata from MusicBrainz when the user presses `A`
2. Caches results in SQLite for instant subsequent lookups
3. Displays artist info in a toggleable bottom pane

## Guardrails

### Must Have
- Respect MusicBrainz rate limit (1 req/sec, proper User-Agent with contact info)
- SQLite caching so repeat lookups never hit the network
- Background fetch (never block the TUI thread)
- Graceful degradation when offline or artist not found

### Must NOT Have
- No Discogs integration in v1 (can be added later)
- No Wikipedia scraping -- use only structured MusicBrainz data
- No new external crate dependencies beyond what's already available (`reqwest`, `serde`, `rusqlite` are all present)

## Task Flow

```
Step 1: MusicBrainz API client (koan-core/src/remote/musicbrainz.rs)
    |
Step 2: Data model + DB schema (koan-core/src/artist_info.rs + db/schema.rs + db/queries/artist_info.rs)
    |
Step 3: Fetch pipeline with caching (koan-core/src/artist_info.rs)
    |
Step 4: TUI integration -- state, widget, keybinding (koan-music/src/tui/)
    |
Step 5: Polish -- error states, empty states, scrolling
```

## Detailed Steps

### Step 1: MusicBrainz API Client

**File:** `koan-core/src/remote/musicbrainz.rs`

Create a MusicBrainz REST client following the `lrclib.rs` pattern.

**Endpoints needed:**
- `GET https://musicbrainz.org/ws/2/artist/?query=artist:{name}&fmt=json&limit=5` -- search by name
- `GET https://musicbrainz.org/ws/2/artist/{mbid}?inc=url-rels+genres+tags&fmt=json` -- full details with relations

**Implementation:**
- Blocking `reqwest` client (matches existing pattern -- the whole app uses `reqwest::blocking`)
- `User-Agent: koan-music/0.3.0 (https://github.com/radiosilence/koan)` -- MusicBrainz requires app name + contact URL
- Rate limiting: track last request timestamp with `std::time::Instant`, sleep if < 1 second since last call
- Custom error enum: `MusicBrainzError { Http, NotFound, RateLimited, BadResponse }`
- Serde response types for search results and artist details

**Key data to extract from API responses:**
```rust
pub struct MusicBrainzArtist {
    pub mbid: String,
    pub name: String,
    pub sort_name: Option<String>,
    pub artist_type: Option<String>,     // "Person", "Group", "Orchestra", etc.
    pub country: Option<String>,          // ISO 3166-1 code
    pub disambiguation: Option<String>,   // e.g. "UK electronic artist"
    pub begin_year: Option<i32>,          // formed/born year
    pub end_year: Option<i32>,            // disbanded/died year (None if active)
    pub genres: Vec<String>,              // from genre/tag relations
    pub urls: Vec<(String, String)>,      // (relation_type, url) e.g. ("wikipedia", "https://...")
}
```

**Acceptance criteria:**
- [ ] `search_artist(name) -> Result<Vec<MusicBrainzArtist>>` returns top matches
- [ ] `get_artist(mbid) -> Result<MusicBrainzArtist>` returns full details with genres + URLs
- [ ] Rate limiting enforced (no more than 1 req/sec)
- [ ] Proper User-Agent header set
- [ ] Unit tests for serde deserialization (use recorded JSON fixtures, same as `lrclib.rs` tests)

### Step 2: Data Model + DB Schema

**Files:**
- `koan-core/src/db/schema.rs` -- add `artist_info_cache` table
- `koan-core/src/db/queries/artist_info.rs` -- cache get/set functions
- `koan-core/src/db/queries/mod.rs` -- add module

**New table:**
```sql
CREATE TABLE IF NOT EXISTS artist_info_cache (
    id          INTEGER PRIMARY KEY,
    artist_name TEXT NOT NULL,
    mbid        TEXT,
    artist_type TEXT,
    country     TEXT,
    disambiguation TEXT,
    begin_year  INTEGER,
    end_year    INTEGER,
    genres      TEXT,           -- JSON array: ["electronic", "ambient"]
    urls        TEXT,           -- JSON array: [["wikipedia","https://..."], ...]
    fetched_at  INTEGER NOT NULL,
    UNIQUE(artist_name)
);
```

Cache keyed on `artist_name` (not track_id like lyrics) because multiple tracks share the same artist. The `UNIQUE(artist_name)` constraint with `ON CONFLICT DO UPDATE` matches the lyrics cache pattern.

Also update the existing `artists` table `mbid` column when we learn a MusicBrainz ID -- this enriches the local DB for future use.

**Query functions** (following `lyrics.rs` pattern):
```rust
pub fn get_cached_artist_info(conn, artist_name) -> Result<Option<ArtistInfo>>
pub fn cache_artist_info(conn, info: &ArtistInfo) -> Result<()>
```

**Acceptance criteria:**
- [ ] Table created in `create_tables()` (idempotent, `IF NOT EXISTS`)
- [ ] `get_cached_artist_info` returns `None` on miss, `Some(ArtistInfo)` on hit
- [ ] `cache_artist_info` upserts (insert or replace on conflict)
- [ ] Genres/URLs stored as JSON text, deserialized on read
- [ ] Unit tests with in-memory SQLite (same pattern as `lyrics.rs` tests)

### Step 3: Fetch Pipeline

**File:** `koan-core/src/artist_info.rs`

Top-level fetch function following the lyrics pipeline pattern:

```rust
pub fn fetch_artist_info(conn: &Connection, artist_name: &str) -> Result<ArtistInfo, ArtistInfoError>
```

**Pipeline:**
1. Check `artist_info_cache` table -- return immediately on hit
2. Search MusicBrainz by artist name
3. Pick best match (exact name match preferred, then first result)
4. Fetch full details with genres + URLs
5. Cache result in SQLite
6. Update `artists.mbid` if we have a matching artist row
7. Return result

**Acceptance criteria:**
- [ ] Cache hit returns instantly (no network)
- [ ] Cache miss fetches from MusicBrainz and caches result
- [ ] "Not found" is cached as a tombstone (avoid re-fetching) -- store with empty fields and a flag, or use a separate sentinel
- [ ] Error types: `Db`, `MusicBrainz`, `NotFound`
- [ ] Public `ArtistInfo` struct with all display fields

### Step 4: TUI Integration

**Files:**
- `koan-music/src/tui/artist_info.rs` -- new: `ArtistInfoState` + `ArtistInfoPanel` widget
- `koan-music/src/tui/app.rs` -- add state, keybinding, tick handler
- `koan-music/src/tui/ui.rs` -- render artist info pane
- `koan-music/src/tui/keys.rs` -- add `A` to hint bar
- `koan-music/src/tui/mod.rs` -- add module

**State** (mirrors `LyricsState`):
```rust
pub struct ArtistInfoState {
    pub result: Option<ArtistInfo>,
    pub artist_name: Option<String>,  // change detection (like lyrics.track_path)
    pub fetching: bool,
    pub scroll_offset: usize,         // artist bios can be long
}
```

**Widget** (`ArtistInfoPanel`):
- Bottom pane (like lyrics is a right pane), toggled with `A`
- Show: artist name (bold), type + country + years, genres, URLs
- Spinner while fetching (reuse same braille spinner pattern)
- "no artist info" on miss
- Scrollable with Up/Down when panel is focused (or just render what fits)

**App integration** (following lyrics pattern exactly):
- `app.artist_info: ArtistInfoState` field
- `app.artist_info_panel: bool` toggle
- `app.artist_info_rx: Option<Receiver<Option<ArtistInfo>>>` for async result
- In `handle_tick()`: check `artist_info_rx.try_recv()`, detect artist change, spawn fetch thread
- `handle_normal_key()`: `KeyCode::Char('A')` toggles `artist_info_panel`
- Background thread named `"koan-artist-info"`, opens its own DB connection (same as lyrics)

**Layout in `ui.rs`:**
When `artist_info_panel` is active, split content area: queue on top (or left), artist info on bottom (or right). If both lyrics AND artist info are active, use a 3-column or stacked layout. Simplest v1: artist info replaces lyrics position when active (they share the right pane slot), or stack vertically.

**Acceptance criteria:**
- [ ] `A` key toggles artist info panel on/off
- [ ] Panel shows spinner while fetching
- [ ] Panel shows artist info on success, "no artist info" on failure
- [ ] Info updates when playing artist changes (same change-detection as lyrics)
- [ ] Background fetch never blocks TUI thread
- [ ] `A` key documented in hint bar

### Step 5: Polish

- Handle edge cases: empty artist name, very long bios, unicode artist names
- "Fetched from MusicBrainz" attribution line (MB requires this)
- Consider a cache TTL (e.g. re-fetch after 30 days) -- but not required for v1, cache is permanent like lyrics
- Test with various artist types: solo artist, group, orchestra, unknown artist

**Acceptance criteria:**
- [ ] No panics on empty/missing data
- [ ] MusicBrainz attribution displayed
- [ ] Works with both local and remote (Subsonic) tracks

## Success Criteria

- [ ] Pressing `A` opens a pane showing the current track's artist info
- [ ] First lookup for an artist hits MusicBrainz (visible spinner), subsequent lookups are instant from cache
- [ ] Rate limiting is respected (verified by log output or manual testing)
- [ ] No new crate dependencies added
- [ ] All new code has unit tests (API deserialization, DB cache round-trip, fetch pipeline)
- [ ] Existing tests still pass (`cargo test --workspace`)

## Files to Create/Modify

| File | Action | Description |
|---|---|---|
| `koan-core/src/remote/musicbrainz.rs` | Create | MusicBrainz API client |
| `koan-core/src/remote/mod.rs` | Modify | Add `pub mod musicbrainz;` |
| `koan-core/src/artist_info.rs` | Create | Fetch pipeline + public types |
| `koan-core/src/lib.rs` | Modify | Add `pub mod artist_info;` |
| `koan-core/src/db/schema.rs` | Modify | Add `artist_info_cache` table |
| `koan-core/src/db/queries/artist_info.rs` | Create | Cache get/set queries |
| `koan-core/src/db/queries/mod.rs` | Modify | Add `pub mod artist_info;` + re-export |
| `koan-music/src/tui/artist_info.rs` | Create | State + panel widget |
| `koan-music/src/tui/mod.rs` | Modify | Add `pub mod artist_info;` |
| `koan-music/src/tui/app.rs` | Modify | Add state, keybinding, tick handler |
| `koan-music/src/tui/ui.rs` | Modify | Render artist info pane |
| `koan-music/src/tui/keys.rs` | Modify | Add `A` to hint bar |

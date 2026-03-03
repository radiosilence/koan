# Plan 07: Non-Tag Metadata

Enriching the player with data beyond what's embedded in audio file tags: similar artists, lyrics, artist bios, external album art, and infinite play / radio mode.

## Summary of Findings

The existing codebase is well-positioned for this. Key advantages:

- **Artists table already has `mbid` column** -- MusicBrainz artist ID, the universal key for cross-referencing every open music API.
- **Albums have `remote_id`** -- Subsonic album IDs, useful since Navidrome already proxies Last.fm data.
- **Subsonic API already supports `getSimilarSongs2`, `getArtistInfo2`, `getLyrics`** -- Navidrome implements all of these (similar songs/artist info require Last.fm integration on the server side).
- **Two-layer config** (config.toml + config.local.toml) is perfect for API keys -- Last.fm API key goes in config.toml (committable), no secrets needed.
- **reqwest is already a dependency** (blocking client) in koan-core. No new HTTP dep needed.
- **No play history table exists yet** -- needed for radio mode's "don't repeat recently played" logic.

What's realistic vs aspirational:
- **Realistic (Phase 1-2):** Lyrics from LRCLIB + embedded tags, similar artists from Last.fm, external album art from Cover Art Archive, Subsonic-proxied metadata.
- **Stretch (Phase 3):** Radio mode, ListenBrainz integration, artist bios display.
- **Aspirational:** Local audio-feature-based similarity (requires DSP analysis, huge effort, skip for now).

---

## API Comparison Matrix

| API | Data Provided | Auth | Rate Limit | Cost | Terms | Quality |
|---|---|---|---|---|---|---|
| **LRCLIB** | Synced + plain lyrics | None | None published | Free | Open, no attribution required (User-Agent encouraged) | Good coverage for popular tracks, community-contributed |
| **Last.fm** | Similar artists (scored), artist bio, album info, artist images, tags | API key (free) | 5 req/sec/IP (averaged over 5min), 100MB data cap | Free (non-commercial) | Non-commercial only, attribution required ("powered by Last.fm" + link), no audio/visual content redistribution | Excellent similarity data, good bios |
| **MusicBrainz** | Artist relationships, MBIDs, release data, Wikipedia/Wikidata links | None (User-Agent required) | 1 req/sec | Free | Open (CC0 for data) | Authoritative IDs, but no "similar artist" endpoint -- relationships are editorial (member-of, collaborated-with), not similarity |
| **Cover Art Archive** | Album art (front/back, 250/500/1200px thumbnails) | None | No rate limits currently | Free | Open (images are user-contributed, various licenses per image) | High quality, keyed by MusicBrainz release MBID |
| **ListenBrainz** | Similar artists (collaborative filtering), recording recommendations, listen history | Token (free account) | 1 req/sec (MetaBrainz standard) | Free | Open (CC0) | Good for users who scrobble to LB; cold-start problem for others |
| **Fanart.tv** | HD artist images, album art, logos | API key (free) | Effectively unlimited (rare 429s) | Free (non-commercial) | Attribution required, must inform users, no commercial use without consent | High quality artist images, keyed by MusicBrainz MBID |
| **Discogs** | Artist bio, images, release data | OAuth or token | 25 req/min unauthed, 60 req/min authed | Free | Commercial use requires agreement | Good data but awkward auth, images require auth |
| **Subsonic (Navidrome)** | Similar songs, artist info/bio/image, lyrics, cover art | Server credentials (existing) | Server-dependent | N/A (self-hosted) | N/A | Proxies Last.fm data; lyrics from embedded + external .lrc files |

**Verdict:** Last.fm + LRCLIB + Cover Art Archive is the sweet spot. All free, open enough for a non-commercial player, and cover the primary use cases. ListenBrainz is a great Phase 3 addition for users who use it. Subsonic endpoints should be leveraged first for remote users since they already have auth configured.

---

## Lyrics Architecture

### Sources (priority order)

1. **Embedded lyrics** -- `ItemKey::Lyrics` via lofty reads USLT (ID3v2), Vorbis LYRICS, and MP4 lyrics tags. Lofty does NOT currently support SYLT (synchronized ID3 lyrics) -- it only gets unsynchronized plain text. For synced lyrics from embedded tags, would need the `id3` crate directly or wait for lofty support.

2. **Sidecar .lrc files** -- Check for `trackname.lrc` next to the audio file. Common convention, zero API calls. Parse with the `lrc` crate.

3. **LRCLIB API** -- `GET https://lrclib.net/api/get?artist_name=X&track_name=Y&album_name=Z&duration=N` -- returns both `syncedLyrics` (LRC format) and `plainLyrics`. Duration matching helps disambiguate. No API key needed.

4. **Subsonic `getLyrics`** -- For remote tracks. `GET /rest/getLyrics?artist=X&title=Y`. Navidrome returns embedded or external .lrc lyrics from the server.

### LRC Format

```
[00:12.00]Line one of lyrics
[00:17.20]Line two of lyrics
[01:15.00]Line three
```

The `lrc` crate (pure Rust, ~no deps beyond regex) parses this into `Vec<(TimeTag, String)>` with `find_timed_line_index(TimeTag)` for O(log n) lookup by playback position.

### Display in TUI

Two display modes:

**A) Synced lyrics panel** -- Scrolling lyrics view synced to playback position. Current line highlighted, +-N lines of context visible. Updates on each tick (~10Hz is plenty). Could be a side panel in Normal mode or a modal overlay.

**B) Plain lyrics view** -- Static scrollable text in the TrackInfo overlay (already exists). Just add a "Lyrics" tab/section.

Implementation approach:
- Add a `LyricsState` to App holding current lyrics (synced or plain) + fetch status.
- On track change, spawn a background fetch: embedded -> sidecar -> LRCLIB -> Subsonic.
- Cache result in DB (see schema below).
- Render as a Paragraph widget with line highlighting via `Style`. The current line gets bold/color, surrounding lines get dimmed.

### Synced Lyrics Rendering

```
   dim     And I find it kind of funny
   dim     I find it kind of sad
 > BOLD    The dreams in which I'm dying
   dim     Are the best I've ever had
   dim     I find it hard to tell you
```

Use the `lrc` crate's `find_timed_line_index()` with current playback position (already available from `SharedPlayerState`). Scroll the paragraph so the active line stays centered.

---

## Recommendation / Radio Mode Design

### Similar Artists (for browsing)

**Data flow:**
1. Current artist -> MusicBrainz MBID (from `artists.mbid` column, or lookup by name)
2. MBID -> Last.fm `artist.getSimilar` -> list of (artist_name, mbid, match_score)
3. Cross-reference with local library: `SELECT id FROM artists WHERE mbid IN (?) OR name IN (?)`
4. Display: "Similar artists in your library: X, Y, Z" in artist info view

**For Subsonic users:** `getArtistInfo2` already returns `<similarArtist>` elements. Use that first, fall back to direct Last.fm.

### Radio Mode (Infinite Play)

This is a **queue feature**, not a player mode. The queue already has a cursor concept; radio mode just auto-appends when the cursor nears the end.

**Architecture:**

```
RadioMode {
    enabled: bool,
    seed: RadioSeed,          // what started the radio
    history: VecDeque<i64>,   // recently played track IDs (avoid repeats)
    history_max: usize,       // e.g. 200
}

enum RadioSeed {
    Artist(i64),              // artist_id
    Track(i64),               // track_id (use its artist + genre)
    Genre(String),
    Queue,                    // seed from whatever's currently playing
}
```

**Track selection algorithm:**

1. Get the current/seed track's artist_id and genre.
2. Query similar artists (cached in DB from Last.fm).
3. Build a candidate pool:
   - Tracks by similar artists in local library (weighted by similarity score)
   - Tracks with matching genre (lower weight)
   - Tracks by the seed artist (some weight, variety is good)
4. Exclude tracks in `history` VecDeque.
5. Weighted random selection from pool.
6. Enqueue 3-5 tracks at a time (batch to avoid per-track API calls).
7. Trigger refill when queue has < 3 unplayed tracks remaining.

**Fallback chain:**
1. Similar artists (Last.fm/Subsonic) -> local tracks by those artists
2. Same genre -> random tracks with matching genre tag
3. Random from library (nuclear fallback)

**For Subsonic:** Use `getSimilarSongs2` directly -- it returns ready-to-play track IDs. No need to do the similarity lookup ourselves.

### Play History Table (needed for radio + future scrobbling)

```sql
CREATE TABLE IF NOT EXISTS play_history (
    id          INTEGER PRIMARY KEY,
    track_id    INTEGER REFERENCES tracks(id),
    played_at   INTEGER NOT NULL,  -- unix timestamp
    duration_ms INTEGER,           -- how long they actually listened
    source      TEXT DEFAULT 'local'
);
CREATE INDEX IF NOT EXISTS idx_play_history_track ON play_history(track_id);
CREATE INDEX IF NOT EXISTS idx_play_history_time ON play_history(played_at);
```

Record a play when >50% of the track has been listened to (same threshold as Last.fm scrobbling rules). This also enables future features: most played, recently played, listening stats.

---

## DB Schema Additions

```sql
-- Cached lyrics
CREATE TABLE IF NOT EXISTS lyrics_cache (
    id          INTEGER PRIMARY KEY,
    track_id    INTEGER REFERENCES tracks(id),
    source      TEXT NOT NULL,         -- 'embedded', 'sidecar', 'lrclib', 'subsonic'
    synced      INTEGER DEFAULT 0,     -- 1 if LRC-format synced lyrics
    content     TEXT NOT NULL,          -- raw lyrics text (LRC format if synced)
    fetched_at  INTEGER NOT NULL,       -- unix timestamp
    UNIQUE(track_id)
);

-- Cached similar artists (from Last.fm or ListenBrainz)
CREATE TABLE IF NOT EXISTS similar_artists (
    id              INTEGER PRIMARY KEY,
    artist_id       INTEGER REFERENCES artists(id),
    similar_artist  TEXT NOT NULL,       -- name (may not be in local library)
    similar_mbid    TEXT,                -- MusicBrainz ID if known
    score           REAL,                -- similarity score 0.0-1.0
    source          TEXT NOT NULL,       -- 'lastfm', 'listenbrainz', 'subsonic'
    fetched_at      INTEGER NOT NULL,
    UNIQUE(artist_id, similar_artist)
);
CREATE INDEX IF NOT EXISTS idx_similar_artist ON similar_artists(artist_id);

-- Cached artist info (bio, images)
CREATE TABLE IF NOT EXISTS artist_info (
    id          INTEGER PRIMARY KEY,
    artist_id   INTEGER REFERENCES artists(id),
    bio         TEXT,                    -- plain text biography
    image_url   TEXT,                    -- URL to artist image
    source      TEXT NOT NULL,           -- 'lastfm', 'musicbrainz', 'subsonic'
    fetched_at  INTEGER NOT NULL,
    UNIQUE(artist_id, source)
);

-- Cached album art URLs (for albums without embedded art)
CREATE TABLE IF NOT EXISTS album_art_cache (
    id          INTEGER PRIMARY KEY,
    album_id    INTEGER REFERENCES albums(id),
    image_url   TEXT NOT NULL,           -- URL to full-size image
    thumb_url   TEXT,                    -- URL to thumbnail (250px or 500px)
    source      TEXT NOT NULL,           -- 'coverartarchive', 'lastfm', 'fanart'
    fetched_at  INTEGER NOT NULL,
    UNIQUE(album_id, source)
);

-- Play history (for radio mode + stats)
CREATE TABLE IF NOT EXISTS play_history (
    id          INTEGER PRIMARY KEY,
    track_id    INTEGER REFERENCES tracks(id),
    played_at   INTEGER NOT NULL,
    duration_ms INTEGER,
    source      TEXT DEFAULT 'local'
);
CREATE INDEX IF NOT EXISTS idx_play_history_track ON play_history(track_id);
CREATE INDEX IF NOT EXISTS idx_play_history_time ON play_history(played_at);
```

**Migration strategy:** `create_tables()` already uses `CREATE TABLE IF NOT EXISTS` everywhere, so new tables are additive. No migration framework needed -- just add them to the existing `execute_batch` call in `schema.rs`. Existing DBs will get the tables on next startup.

**MusicBrainz MBID population:** The `artists.mbid` column already exists but is never populated for local tracks. Add a background MBID lookup task:
- On first play of an artist, query MusicBrainz: `GET /ws/2/artist/?query=artist:NAME&fmt=json`
- Store the best-match MBID in `artists.mbid`
- This MBID is the universal key for Cover Art Archive, Last.fm (via mbid param), ListenBrainz, and Fanart.tv

---

## Caching Strategy

### TTL Policy

| Data Type | TTL | Rationale |
|---|---|---|
| Lyrics | Forever (no expiry) | Lyrics don't change. Re-fetch only if track metadata changes. |
| Similar artists | 30 days | Similarity scores shift slowly as listening data evolves. |
| Artist info/bio | 30 days | Bios are stable, images may update. |
| Album art URLs | 90 days | Cover art is essentially permanent. |
| MBIDs | Forever | Canonical IDs don't change. |

### Fetch Strategy

**Lazy, not eager.** Never bulk-fetch during library scan. Fetch metadata when:
1. A track starts playing (lyrics, album art)
2. A user views an artist page (similar artists, artist bio)
3. Radio mode needs candidates (similar artists for seed)

**Background thread:** Metadata fetches happen on a dedicated thread (or use the existing reqwest blocking client on a spawned thread). Results are sent back to the main thread via crossbeam channel (already in deps).

### Image Caching

Downloaded images (album art, artist photos) go to `~/.config/koan/metadata-cache/`:
- `art/{album_id}.jpg` -- album covers
- `artists/{artist_id}.jpg` -- artist images

SQLite stores the URL; actual image bytes live on disk. The cover art rendering pipeline already loads from disk (for souvlaki temp files), so this fits.

### Rate Limiter

Simple token bucket per API:
```rust
struct RateLimiter {
    last_request: Instant,
    min_interval: Duration,
}
```

- Last.fm: 200ms between requests (5/sec)
- MusicBrainz: 1000ms between requests (1/sec)
- LRCLIB: 100ms (generous, they have no published limit)
- Cover Art Archive: 200ms (be polite despite no stated limit)

Implement as a simple `sleep_until(last_request + min_interval)` before each request. No need for a fancy async rate limiter -- these are background fetches, latency doesn't matter.

---

## TUI Display Considerations

### Lyrics Panel

Option A: **Side panel** -- Split the main area into queue (left) + lyrics (right) when lyrics are available. Toggled with a keybind (e.g., `L`).

Option B: **Tab in TrackInfo modal** -- Add a "Lyrics" tab to the existing TrackInfo overlay. Less intrusive but loses the "always visible synced lyrics" experience.

Recommendation: **Start with Option A (side panel).** It's the killer feature for synced lyrics. The existing layout already splits into queue + transport; adding a conditional right panel is straightforward in ratatui.

### Artist Info / Similar Artists

Display in the TrackInfo overlay or a new ArtistInfo overlay:
- Bio text: Paragraph widget with word wrap, scrollable
- Similar artists: List, highlight ones present in local library
- Artist image: CoverArt widget (already exists) repurposed

### Rich Text in TUI

Last.fm bios contain HTML/wiki markup. Strip to plain text before storing:
- Remove `<a>` tags but keep link text
- Convert `<br>` and `<p>` to newlines
- Strip all other HTML tags
- Store plain text in DB

Use a simple regex-based strip or the `ammonia` crate if we want to be thorough. Don't bother with any markup rendering in the TUI -- plain text is fine for bios.

---

## Implementation Phases

### Phase 1: Lyrics (Medium effort, high impact)

1. Add `lyrics_cache` table to schema
2. Read embedded lyrics via `tag.get_string(&ItemKey::Lyrics)` in metadata.rs
3. Check for sidecar `.lrc` files
4. Add LRCLIB client (simple GET endpoint, ~50 lines)
5. Add `getLyrics` to Subsonic client
6. LRC parser: add `lrc` crate dependency
7. Lyrics fetch pipeline: embedded -> sidecar -> LRCLIB/Subsonic -> cache
8. TUI: Add synced lyrics panel with current-line highlighting
9. Keybind to toggle lyrics panel (e.g., `L`)

**New deps:** `lrc` crate (~minimal)
**Effort:** ~2-3 days

### Phase 2: Similar Artists + External Art (Medium effort, medium impact)

1. Add `similar_artists`, `artist_info`, `album_art_cache` tables
2. Add Last.fm client module (API key in config.toml, `artist.getSimilar`, `artist.getInfo`, `album.getInfo`)
3. Add `getArtistInfo2` and `getSimilarSongs2` to Subsonic client
4. MBID population: MusicBrainz artist lookup on first encounter
5. Cover Art Archive client: simple GET by release MBID
6. Background metadata fetch on track play / artist view
7. Rate limiter utility
8. Config additions:

```toml
[metadata]
lastfm_api_key = ""           # in config.toml (not a secret)
enabled_sources = ["lastfm", "musicbrainz", "lrclib", "coverartarchive"]
```

**New deps:** None (reqwest already available, serde handles JSON)
**Effort:** ~3-4 days

### Phase 3: Radio Mode (High effort, high impact)

1. Add `play_history` table
2. Record plays (>50% listened threshold)
3. Radio mode state machine in queue module
4. Track selection algorithm (similar artists -> genre -> random fallback)
5. Auto-enqueue logic (refill when < 3 tracks remaining)
6. TUI: Radio mode indicator in transport bar, toggle keybind
7. Subsonic shortcut: use `getSimilarSongs2` for remote libraries

**Effort:** ~4-5 days

### Phase 4: Polish + ListenBrainz (Lower priority)

1. ListenBrainz similar artists (collaborative filtering -- better than Last.fm for some users)
2. ListenBrainz scrobbling (submit plays)
3. Fanart.tv artist images (higher quality than Last.fm)
4. Artist bio display in TUI
5. Wikipedia links from MusicBrainz relationships

**Effort:** ~3-4 days

---

## Legal / Terms of Service Considerations

### Last.fm

- **Non-commercial only.** koan is MIT-licensed open source, not monetized -- this is fine.
- **Attribution required.** Must show "powered by Last.fm" or "data from Last.fm" somewhere. In a TUI, a small note in the artist info view or about screen suffices.
- **Link back required.** When showing artist/album data from Last.fm, link to the Last.fm page. In a TUI, display the URL as text (user can cmd-click in most terminals).
- **100MB data cap.** Only cache what's needed, respect TTLs. Should be well under this for a personal music player.
- **No redistributing audio/visual content.** Don't cache and serve Last.fm images to other users. Local caching for the player's own display is fine (same as every other Last.fm client).

### LRCLIB

- No terms of service. No API key. No attribution required.
- Courtesy: set a User-Agent header identifying koan.
- Community-contributed database -- quality varies, but the matching-by-duration helps.

### MusicBrainz / Cover Art Archive

- Data is CC0 (public domain). No attribution required (but encouraged).
- Must provide a meaningful User-Agent with contact info.
- Rate limit: 1 req/sec. Easy to respect.
- Cover Art Archive images have per-image licenses (set by uploader). Most are permissive but technically each image could differ.

### ListenBrainz

- Open source, CC0 data.
- Requires user authentication for recommendations (user must have a ListenBrainz account).
- Rate limit: 1 req/sec (shared with MusicBrainz).

### Fanart.tv

- **Non-commercial only** (like Last.fm).
- Must inform users about the source.
- Must request users contribute artwork.
- API key required (free to obtain).
- Project API key + optional personal user key for better rate limits.

### Practical Implications

- **Ship with no API keys.** User must provide their own Last.fm API key (free, takes 30 seconds to get). LRCLIB and Cover Art Archive need no keys at all.
- **OR ship a project API key** for Last.fm/Fanart.tv. This is what most open-source players do (Navidrome, Strawberry, etc.). Terms allow it for non-commercial use.
- Document attribution requirements in the README.
- All metadata features should be opt-in (disabled by default) with a config toggle.

---

## Config Schema Design

```toml
[metadata]
# Enable external metadata fetching (default: false)
enabled = false

# Last.fm API key — get one free at https://www.last.fm/api/account/create
# Not a secret, safe to commit to dotfiles.
lastfm_api_key = ""

# ListenBrainz username (optional, for personalized recommendations)
listenbrainz_username = ""

# Fanart.tv API key (optional, for HD artist images)
fanart_api_key = ""

# Which metadata to fetch. Remove items to disable specific lookups.
# Options: "lyrics", "similar_artists", "artist_info", "album_art"
features = ["lyrics", "similar_artists", "album_art"]

# Show synced lyrics panel by default when available
lyrics_panel = true
```

API keys go in `config.local.toml` if the user considers them private, or `config.toml` if they don't care (Last.fm API keys are not secrets -- they're app identifiers, not user credentials).

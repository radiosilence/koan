# kōan

A music player for people who give a shit about audio quality.

## Philosophy

The name is a zen kōan — a question with no logical answer. Like "why is every music player on macOS either abandonware, subscription bullshit, or electron garbage?"

kōan doesn't try to be clever. It plays your music correctly, indexes it fast, and gets out of the way. Bit-perfect or go home.

## Core Architecture

**Swift UI shell + Rust audio/database core.** The UI is native SwiftUI for tight macOS integration (media keys, menu bar, notifications, system audio routing). The heavy lifting — database, indexing, audio decoding, format string parsing, file operations — lives in a Rust dylib linked via C FFI.

```
┌─────────────────────────────────────┐
│          SwiftUI Frontend           │
│  Library browser, album art, queue  │
├─────────────────────────────────────┤
│         Swift ↔ Rust FFI            │
├──────────┬──────────┬───────────────┤
│  Audio   │ Database │  File Ops     │
│ Engine   │ (SQLite) │  & Indexing   │
│ CoreAudio│  FTS5    │  Symphonia    │
│ exclusive│          │  + metadata   │
└──────────┴──────────┴───────────────┘
```

## Audio Engine

### Bit-Perfect Playback

- CoreAudio exclusive mode (AudioUnit, not AVAudioPlayer)
- Integer-mode DAC output when hardware supports it
- No sample rate conversion — match source to output device
- Automatic device sample rate switching (44.1→96→192 etc.)
- Zero software volume by default (pure digital passthrough)
- Optional software volume with 64-bit float processing

### Gapless Playback

- Pre-decode next track into ring buffer while current plays
- Read-ahead threading: decode thread fills buffer, audio thread drains it
- Crossfade as an option, not a hack to hide gaps
- Handle gapless albums (live recordings, concept albums) correctly via embedded gapless metadata

### Format Support (via Symphonia + platform decoders)

- FLAC (native, including 24/32-bit hi-res)
- ALAC (via CoreAudio)
- MP3 (CBR/VBR, gapless via LAME header)
- Opus
- Vorbis (OGG)
- AAC/M4A (via CoreAudio)
- WavPack
- WAV/AIFF (PCM)
- DSD (DoP passthrough to compatible DACs)

### ReplayGain

- Read existing RG tags (track and album gain)
- Album mode by default (preserve dynamic range within albums)
- R128/EBU loudness normalization as alternative
- Limiter to prevent clipping after gain application
- Scan and write RG tags to files

## Database & Indexing

### Speed Target

- 50,000 tracks indexed in under 10 seconds (metadata only, no audio analysis)
- Incremental re-index on filesystem changes (FSEvents on macOS)
- Full-text search across all metadata fields (SQLite FTS5)
- Startup to music playing: under 2 seconds on warm cache

### Schema Design

SQLite with WAL mode. Single writer, concurrent readers. Metadata stored as structured columns, not blobs.

```sql
-- Core tables
artists (id, name, sort_name, mbid)
albums (id, artist_id, title, date, original_date, label, catalog, codec, total_discs, total_tracks, mbid)
tracks (id, album_id, artist_id, disc, track, title, duration_ms, path, format, sample_rate, bit_depth, channels, bitrate, size_bytes, mbid)

-- Denormalized for speed
album_artists (album_id, artist_id, role)  -- handle splits, features, VA
genres (id, name)
track_genres (track_id, genre_id)

-- Playback state
play_stats (track_id, play_count, last_played, skip_count, rating)
playback_state (current_track_id, position_ms, queue_json)

-- Library management
library_folders (id, path, last_scan, watch)
scan_cache (path, mtime, size, track_id)  -- skip unchanged files on re-scan
```

### Watch Mode

- FSEvents for real-time filesystem monitoring
- Debounced batch updates (don't re-index on every write during a copy operation)
- Handle volume mount/unmount gracefully (Turtlehead goes to sleep sometimes)

## Library Browser

### Views (customizable, fb2k-style)

Each view is a tree definition using format strings:

```
by artist/album:    [%album artist%] → ['('$left(%date%,4)')' ]%album% [%codec%] → tracks
by genre:           %genre% → [%album artist% - ]%album% → tracks
by year:            %date% → [%album artist% - ]%album% → tracks
by quality:         %codec% → [%sample_rate%/%bit_depth%] → %album artist% - %album% → tracks
by label:           %label% → %album artist% - %album% ['('$left(%date%,4)')'] → tracks
by added:           $date(%added%) → %album artist% - %album% → tracks
by folder:          literal filesystem tree
```

Users can define custom views with the same format string syntax.

### Faceted Browsing

ReFacets-style multi-pane filtering:

- Genre pane | Album Artist pane | Album pane
- Click genre → artist list filters → album list filters → playlist shows tracks
- Each pane is configurable (any metadata field)

### Album Art

- Embedded art (FLAC, MP3 ID3, M4A)
- `cover.jpg` / `cover.png` / `folder.jpg` in album directory
- Configurable art search patterns
- Art cache (thumbnails stored in DB as blobs for instant grid view)
- High-res art display in now-playing panel
- Grid view option for album browsing (like iTunes used to be before it sucked)

## Playlist Columns

Customizable columns with format strings. Built-in columns:

| Column       | Format                    | Notes                          |
| ------------ | ------------------------- | ------------------------------ |
| #            | `%tracknumber%`           | Disc-aware: `0101`, `0205`     |
| Artist       | `%artist%`                | Track artist, not album artist |
| Album Artist | `%album artist%`          |                                |
| Title        | `%title%`                 |                                |
| Album        | `%album%`                 |                                |
| Date         | `$left(%date%,4)`         | Year only                      |
| Duration     | `%length%`                | `mm:ss` format                 |
| Codec        | `%codec%`                 | FLAC, MP3, OPUS etc.           |
| Bitrate      | `%bitrate%`               | kbps                           |
| Sample Rate  | `%samplerate%`            | Hz                             |
| Bit Depth    | `$info(bitspersample)`    | 16, 24, 32                     |
| Genre        | `%genre%`                 |                                |
| Label        | `%label%`                 |                                |
| Path         | `%path%`                  | Full file path                 |
| Rating       | `%rating%`                | Stars                          |
| Play Count   | `%play_count%`            |                                |
| ReplayGain   | `%replaygain_track_gain%` | dB                             |

Users define custom columns with arbitrary format strings. Columns are reorderable and resizable.

## File Operations

### Format String Engine

fb2k-compatible title formatting language. This is non-negotiable — existing users have muscle memory OR if it makes more sense, beets style tagging.

**Functions:**

- `$if(cond,then,else)`, `$if2(a,b)`, `$if3(a,b,c)`
- `$left(str,n)`, `$right(str,n)`, `$pad(str,n)`, `$pad_right(str,n)`
- `$num(n,digits)` — zero-padded number
- `$replace(str,from,to)`, `$trim(str)`
- `$lower(str)`, `$upper(str)`, `$caps(str)`
- `$directory(path)`, `$directory_path(path)`, `$ext(path)`, `$filename(path)`
- `$stricmp(a,b)` — case-insensitive compare
- `$date(timestamp)` — format date
- `$info(field)` — technical metadata
- `$insert(str,pos)`, `$div(a,b)`, `$mod(a,b)`

**Variables:**

- `%field%` — metadata field lookup
- `%<field>%` — remapped field (e.g., `%<artist>%` = album artist for non-VA, per-track for VA)
- `[...]` — conditional: only output if all fields inside resolve
- `'...'` — literal text (no field expansion)

### Rename/Move Operations

- Preview before executing (show old → new path)
- Batch operations with progress
- Move other files (cover.jpg, .cue, .log) with the music
- Remove empty directories after move
- Undo support (keep a move log, reverse it)
- Presets (save named format strings for common operations)

### Beets Integration (optional)

- Shell out to `beet import` for MusicBrainz matching
- Read beets database for cross-reference
- Or: native MusicBrainz Picard-style lookup via the MB API (acoustid fingerprinting)
- This could be a v2 feature — start with manual metadata + format string operations

## UI Design

### Aesthetic

Minimal, dark, typographic. Think "what if Dieter Rams designed a music player."

- System-native dark mode, respects macOS appearance
- No skeuomorphism, no gratuitous gradients
- Album art is the colour — UI is monochrome around it
- Good typography: SF Pro for UI, SF Mono for technical info (bitrate, sample rate)
- Dense but not cramped — information-rich without clutter
- Subtle animations (crossfade between album art, smooth scrolling)

### Layout

```
┌──────────────────────────────────────────────┐
│ ◀ ▶ ■  ━━━━━━━━━●━━━━━━━  2:34/5:12  🔊    │  <- transport bar
├────────────┬─────────────────────────────────┤
│            │                                 │
│  Library   │   Playlist / Track List         │
│  Browser   │                                 │
│  (tree or  │   columns, sortable, format-    │
│   facets)  │   string-driven                 │
│            │                                 │
│            │                                 │
│            │                                 │
├────────────┤                                 │
│            │                                 │
│  Album Art │                                 │
│  (large)   │                                 │
│            │                                 │
├────────────┼─────────────────────────────────┤
│  Now Playing: Artist - Title | Album | FLAC  │
│  44.1kHz/16-bit → Built-in Output (matched)  │  <- audio chain status
└──────────────────────────────────────────────┘
```

The audio chain status bar is key — always shows: source format → sample rate → bit depth → output device → whether exclusive mode is active. Audiophiles want to _see_ that their signal path is clean.

### Keyboard-Driven

- Vim/Helix-style navigation (j/k, /, g, G)
- Space = play/pause
- Type-to-search in any pane
- Command palette (⌘K) for everything
- Customizable keybindings

### Media Keys

- Native macOS media key handling (MediaRemote framework)
- Now Playing info in Control Center
- Lock screen controls
- AirPlay output selection

## Subsonic/Navidrome Support

### Streaming

- Subsonic API v1.16+ / OpenSubsonic
- Stream original quality (no transcoding) when on local network
- Configurable transcode quality for remote (opus 128k, etc.)
- Pre-buffer next track for gapless streaming
- Offline cache: pin albums/playlists for offline playback
- Scrobble plays back to Navidrome

### Hybrid Library

- Unified view: local files + Navidrome library appear as one
- Visual indicator for local vs. streaming tracks
- Smart downloading: stream first, background-download to local library
- Conflict resolution: local file wins if same album exists in both

### Sync

- Two-way playlist sync with Navidrome
- Star ratings sync
- Play count sync
- Background sync on app launch

## Performance Targets

| Metric                       | Target       |
| ---------------------------- | ------------ |
| Cold start to UI ready       | < 1s         |
| Library scan (50k tracks)    | < 10s        |
| Incremental rescan           | < 1s         |
| Search latency               | < 50ms       |
| Track transition (gapless)   | < 1ms gap    |
| Memory (100k track library)  | < 200MB      |
| Album art grid (1000 albums) | 60fps scroll |

## Build & Distribution

- Swift Package Manager for the app shell
- Cargo for the Rust core (built as `libkoan.dylib`)
- `uniffi` or `swift-bridge` for FFI generation
- Distribute via Homebrew cask (no App Store — we want exclusive audio mode)
- Auto-update via Sparkle
- Notarized and signed for Gatekeeper

## What This Isn't

- Not a streaming service client (Spotify/Tidal/etc. — use their apps)
- Not a DJ tool (no BPM detection, beat matching, crossfade mixing)
- Not a podcast player
- Not trying to manage your entire media library (photos, videos)
- Not an audio editor

## Stretch Goals (v2+)

- **DSP chain**: parametric EQ, room correction (convolution), crossfeed for headphones
- **Spectral analysis**: Should warn if FLAC is fake transcoded MP3s
- **MusicBrainz integration**: auto-tag, auto-fetch album art, acoustid fingerprinting
- **Smart playlists**: query-based dynamic playlists (`genre:post-rock AND year:>2020 AND rating:>=4`)
- **Audio analysis**: waveform display, spectrum analyzer, DR meter
- **Multiple output devices**: route different playlists to different DACs
- **iOS companion**: AirPlay 2 receiver + remote control
- **Last.fm scrobbling**
- **Listenbrainz integration**
- **CUE sheet support** for single-file rips
- **SACD/DSD native playback** (not just DoP)

---

_Because life's too short for audio players that can't even do gapless right._

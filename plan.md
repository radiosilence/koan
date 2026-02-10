# kōan — Implementation Plan

## Context

Every music player on macOS is either abandonware, subscription garbage, or Electron trash. kōan is a native SwiftUI shell over a Rust core that does bit-perfect audio playback, fast library indexing, and fb2k-style customization. The name is a zen kōan — a question with no logical answer, like "why can't anyone make a decent music player?"

This plan gets us from empty repo to playing FLAC files correctly as fast as possible, then layers features. Each phase is independently demoable.

---

## Architecture Summary

```
SwiftUI Shell (native macOS)
    ├── UniFFI bindings ──→ Control plane (library, search, playlists, commands)
    └── Raw C FFI ────────→ Audio data plane (render callback, state reads)
            │
            ▼
    Rust Core (koan-core)
    ├── audio/    CoreAudio HAL wrapper, ring buffer, gapless engine
    ├── db/       SQLite + FTS5, WAL mode
    ├── index/    Symphonia metadata, lofty tags, FSEvents watcher
    ├── format/   fb2k-style format string engine
    └── player/   Playback state machine, queue management
```

**Key insight:** PCM audio data NEVER crosses the FFI boundary. It flows entirely within Rust: file → Symphonia → rtrb ring buffer → CoreAudio render callback → DAC. Swift only sends commands and reads state.

### Crate Stack

| Need | Crate | Version/Notes |
|------|-------|---------------|
| Audio decoding | `symphonia` | FLAC/ALAC/MP3/Vorbis/WAV/AAC-LC native |
| CoreAudio HAL | `objc2-core-audio` | Raw bindings, we build safe wrapper on top |
| SQLite + FTS5 | `rusqlite` | `bundled-full` feature, WAL mode |
| File watching | `notify` | FSEvents backend on macOS |
| Metadata tags | `lofty` | Read/write all tag formats, ReplayGain, MBID |
| Ring buffer | `rtrb` | Wait-free SPSC, audio-thread safe |
| ReplayGain scan | `ebur128` | EBU R128 compliant loudness measurement |
| FFI (control) | `uniffi` | Proc macro bindings → Swift |
| FFI (audio) | `cbindgen` | Zero-overhead C headers |

### Project Layout

```
koan/
├── Cargo.toml                      # workspace root
├── Makefile                        # orchestrate: cargo build → xcframework → swift build
├── crates/
│   ├── koan-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── audio/
│   │       │   ├── mod.rs
│   │       │   ├── engine.rs       # AudioEngine: owns AUHAL unit, render callback
│   │       │   ├── device.rs       # Device enumeration, selection, hog mode
│   │       │   ├── format.rs       # Sample rate switching, integer mode, physical format
│   │       │   ├── buffer.rs       # rtrb ring buffer wrapper, pre-decode logic
│   │       │   └── replaygain.rs   # ebur128 integration, gain application
│   │       ├── db/
│   │       │   ├── mod.rs
│   │       │   ├── schema.rs       # Table definitions, migrations
│   │       │   ├── queries.rs      # Library queries, search, stats
│   │       │   └── connection.rs   # WAL setup, reader pool
│   │       ├── index/
│   │       │   ├── mod.rs
│   │       │   ├── scanner.rs      # Directory walker, metadata extraction
│   │       │   ├── watcher.rs      # FSEvents via notify, debounced updates
│   │       │   └── metadata.rs     # lofty integration, tag reading/writing
│   │       ├── format/
│   │       │   ├── mod.rs
│   │       │   ├── parser.rs       # Format string tokenizer
│   │       │   ├── eval.rs         # Format string evaluator
│   │       │   └── functions.rs    # $if, $left, $right, $pad, etc.
│   │       └── player/
│   │           ├── mod.rs
│   │           ├── state.rs        # PlaybackState enum, transitions
│   │           ├── queue.rs        # Play queue, shuffle, repeat
│   │           └── commands.rs     # Lock-free command channel (play/pause/seek/next)
│   │
│   ├── koan-ffi/                   # UniFFI control plane
│   │   ├── Cargo.toml
│   │   ├── src/lib.rs              # #[uniffi::export] wrappers
│   │   └── uniffi.toml
│   │
│   └── koan-audio-ffi/             # Raw C FFI data plane
│       ├── Cargo.toml              # crate-type = ["staticlib"]
│       ├── src/lib.rs              # extern "C" fns: init, state reads, commands
│       └── cbindgen.toml
│
├── swift/
│   ├── Package.swift               # SPM package
│   └── Sources/
│       ├── Koan/                   # SwiftUI app (@main)
│       │   ├── KoanApp.swift
│       │   ├── Views/
│       │   │   ├── ContentView.swift
│       │   │   ├── TransportBar.swift
│       │   │   ├── LibraryBrowser.swift
│       │   │   ├── TrackList.swift
│       │   │   ├── AlbumArtView.swift
│       │   │   ├── NowPlayingBar.swift
│       │   │   └── AudioChainStatus.swift
│       │   ├── ViewModels/
│       │   │   ├── PlayerViewModel.swift
│       │   │   ├── LibraryViewModel.swift
│       │   │   └── SearchViewModel.swift
│       │   └── Services/
│       │       ├── AudioBridge.swift    # Wraps C FFI for audio state
│       │       ├── MediaKeyHandler.swift
│       │       └── NowPlayingService.swift
│       └── KoanRust/               # Binary target wrapping xcframework
│
├── scripts/
│   ├── build.sh                    # Full build: cargo + xcframework + swift
│   ├── build-rust.sh               # Just cargo build for both FFI crates
│   ├── generate-bindings.sh        # uniffi-bindgen + cbindgen
│   └── dev.sh                      # Watch mode: rebuild on changes
│
└── docs/
    └── (auto-generated from this plan, kept minimal)
```

---

## Phase 0: Project Scaffolding

**Goal:** Empty app that compiles. Rust workspace + Swift package + FFI pipeline proven end-to-end.

### Steps

1. **Init git repo, Cargo workspace, Swift package**
   - `Cargo.toml` workspace with three crates
   - `swift/Package.swift` with binary target dependency
   - `.gitignore` for build artifacts, xcframework, `.build/`

2. **Prove the FFI pipeline**
   - `koan-ffi`: Export a single function via UniFFI: `fn version() -> String`
   - `koan-audio-ffi`: Export a single function via C FFI: `extern "C" fn koan_ping() -> i32`
   - Build script that compiles both → static lib → xcframework
   - Swift app imports both, calls both, displays result in a SwiftUI Text view

3. **Build system**
   - `Makefile` with targets: `build-rust`, `generate-bindings`, `build-xcframework`, `build-swift`, `run`, `clean`
   - Scripts in `scripts/` for the heavy lifting
   - Verify: `make run` launches a window showing "kōan v0.1.0" + ping result

### Deliverable
A macOS window that proves Swift ↔ Rust communication works in both directions (UniFFI + C FFI).

---

## Phase 1: Play a FLAC File

**Goal:** Select a FLAC file → decode with Symphonia → output via CoreAudio. No library, no database, just audio.

### Rust: Audio Engine (`koan-core/src/audio/`)

#### `engine.rs` — Core audio output
```rust
pub struct AudioEngine {
    audio_unit: AudioUnit,          // AUHAL output unit
    ring_buffer: rtrb::Consumer<f32>,
    device_sample_rate: AtomicU32,  // current device rate
    state: AtomicU8,                // Playing/Paused/Stopped
}
```
- Create AUHAL output AudioUnit targeting default device
- Set render callback that drains the rtrb ring buffer
- Handle format negotiation (match source sample rate to device)
- Start/stop audio unit

#### `device.rs` — Device management
```rust
pub struct AudioDevice { id: AudioDeviceID, name: String, sample_rates: Vec<f64> }
pub fn list_devices() -> Vec<AudioDevice>
pub fn set_device_sample_rate(device: AudioDeviceID, rate: f64) -> Result<()>
pub fn set_hog_mode(device: AudioDeviceID, hog: bool) -> Result<()>
```
- Enumerate output devices via `kAudioHardwarePropertyDevices`
- Get/set nominal sample rate via `kAudioDevicePropertyNominalSampleRate`
- Hog mode via `kAudioDevicePropertyHogMode`

#### `buffer.rs` — Decode pipeline
```rust
pub struct DecodeBuffer {
    producer: rtrb::Producer<f32>,
    // decode thread handle
}
```
- Spawn decode thread that: opens file → Symphonia FormatReader → decode packets → push f32 samples to rtrb producer
- Handle EOF → signal engine
- Pre-decode support (fill buffer before starting playback)

### Rust: Player State (`koan-core/src/player/`)

#### `state.rs`
```rust
pub enum PlaybackState { Stopped, Playing, Paused }
pub struct NowPlaying {
    pub path: String,
    pub format: String,       // "FLAC"
    pub sample_rate: u32,     // 44100, 96000, etc.
    pub bit_depth: u16,       // 16, 24, 32
    pub channels: u16,
    pub duration_ms: u64,
    pub position_ms: AtomicU64,
}
```

#### `commands.rs`
```rust
pub enum PlayerCommand { Play(PathBuf), Pause, Resume, Stop, Seek(u64) }
// Lock-free SPSC channel for commands: Swift → audio engine
```

### FFI Layer

#### `koan-audio-ffi/src/lib.rs` (C FFI)
```c
// Exported C functions:
void koan_init(void);
void koan_shutdown(void);
int32_t koan_play(const char* path);
void koan_pause(void);
void koan_resume(void);
void koan_stop(void);
void koan_seek(uint64_t position_ms);

// State reads (called from UI thread, reads atomics):
uint64_t koan_get_position_ms(void);
uint8_t koan_get_state(void);  // 0=stopped, 1=playing, 2=paused
KoanNowPlaying koan_get_now_playing(void);  // #[repr(C)] struct
```

### Swift: Minimal UI

- File → Open dialog to pick a FLAC file
- Transport: play/pause button, seek slider, position label
- Now Playing: filename, format, sample rate, bit depth
- Audio chain status: source format → device → sample rate match indicator

### Deliverable
Pick a FLAC file, hear it play through your DAC with correct sample rate. See the signal chain.

---

## Phase 2: Library Database + Scanner

**Goal:** Point at a folder → scan all music files → store metadata in SQLite → browse in SwiftUI.

### Rust: Database (`koan-core/src/db/`)

#### `schema.rs`
```sql
-- Full schema from spec:
artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort_name TEXT, mbid TEXT)
albums (id INTEGER PRIMARY KEY, title TEXT NOT NULL, date TEXT, original_date TEXT,
        label TEXT, catalog TEXT, total_discs INTEGER, total_tracks INTEGER, mbid TEXT)
album_artists (album_id INTEGER, artist_id INTEGER, role TEXT DEFAULT 'artist')
tracks (id INTEGER PRIMARY KEY, album_id INTEGER, artist_id INTEGER,
        disc INTEGER, track INTEGER, title TEXT NOT NULL,
        duration_ms INTEGER, path TEXT UNIQUE NOT NULL,
        format TEXT, sample_rate INTEGER, bit_depth INTEGER,
        channels INTEGER, bitrate INTEGER, size_bytes INTEGER, mbid TEXT)
genres (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL)
track_genres (track_id INTEGER, genre_id INTEGER)
play_stats (track_id INTEGER PRIMARY KEY, play_count INTEGER DEFAULT 0,
            last_played TEXT, skip_count INTEGER DEFAULT 0, rating INTEGER)
playback_state (id INTEGER PRIMARY KEY CHECK(id=1),
                current_track_id INTEGER, position_ms INTEGER, queue_json TEXT)
library_folders (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL,
                 last_scan TEXT, watch INTEGER DEFAULT 1)
scan_cache (path TEXT PRIMARY KEY, mtime INTEGER, size INTEGER, track_id INTEGER)

-- FTS5 virtual table
tracks_fts (title, artist_name, album_title, genre_names)
```

#### `connection.rs`
- WAL mode + `synchronous=NORMAL`
- Single writer `Arc<Mutex<Connection>>`
- Reader connections cloned as needed
- Prepared statement cache enabled

#### `queries.rs`
- `insert_track()`, `insert_album()`, `insert_artist()` — upsert semantics
- `search(query: &str) -> Vec<TrackResult>` — FTS5 MATCH query
- `get_albums_by_artist()`, `get_tracks_by_album()`, `get_all_artists()`
- `get_library_stats()` — track count, total duration, format breakdown

### Rust: Indexer (`koan-core/src/index/`)

#### `scanner.rs`
- Walk directory tree (rayon parallel iterator for speed)
- Filter by extension: flac, mp3, m4a, ogg, opus, wav, aiff, wv, ape
- Check `scan_cache` — skip if mtime+size unchanged
- Extract metadata via `lofty`
- Group tracks into albums by directory + album tag
- Batch insert in single transaction

#### `metadata.rs`
- Wrapper around `lofty` for unified tag access
- Extract: title, artist, album artist, album, disc/track number, date, genre, label, MBID
- Extract: duration, format, sample rate, bit depth, channels, bitrate
- Extract: embedded album art (first picture, store thumbnail in DB)
- Extract: ReplayGain tags (track gain, album gain, peak)

#### `watcher.rs`
- `notify` crate with FSEvents backend
- Debounce: 2 second window after last event before re-scanning changed directory
- Handle: create, modify, delete, rename events
- On volume mount/unmount: re-scan relevant library folders

### FFI Layer (UniFFI)

```rust
#[uniffi::export]
fn add_library_folder(path: String) -> Result<(), LibraryError>;
#[uniffi::export]
fn scan_library() -> Result<ScanResult, LibraryError>;
#[uniffi::export]
fn search(query: String) -> Vec<TrackInfo>;
#[uniffi::export]
fn get_artists() -> Vec<ArtistInfo>;
#[uniffi::export]
fn get_albums(artist_id: Option<i64>) -> Vec<AlbumInfo>;
#[uniffi::export]
fn get_tracks(album_id: i64) -> Vec<TrackInfo>;
```

### Swift: Library Browser

- Settings panel to add/remove library folders
- Artist list → Album list → Track list (3-pane browser)
- Track list with columns: #, Title, Artist, Album, Duration, Format
- Double-click track to play
- Search bar with live results (FTS5)
- Scan progress indicator

### Deliverable
Add a music folder, see it scan, browse artists/albums/tracks, double-click to play.

---

## Phase 3: Gapless Playback + Queue

**Goal:** Play queue with gapless transitions. Pre-decode next track.

### Rust Changes

#### `buffer.rs` — Gapless pre-decode
- When current track reaches last N seconds, start decoding next track into a second ring buffer
- On track transition: swap buffers atomically, zero gap
- Read LAME gapless header (MP3), encoder delay/padding metadata
- Trim silence samples at track boundaries

#### `queue.rs` — Play queue
```rust
pub struct PlayQueue {
    tracks: Vec<TrackId>,
    current_index: AtomicUsize,
    repeat: AtomicU8,    // Off, One, All
    shuffle: bool,
    shuffle_order: Vec<usize>,
}
```
- Next/previous navigation
- Shuffle (Fisher-Yates on indices, not destructive)
- Repeat modes: off, one, all
- Add/remove/reorder tracks
- "Play album" / "Play artist" convenience methods

#### `commands.rs` — Extended commands
```rust
pub enum PlayerCommand {
    // ... existing ...
    Next, Previous,
    SetQueue(Vec<TrackId>),
    QueueAppend(Vec<TrackId>),
    SetRepeat(RepeatMode),
    SetShuffle(bool),
}
```

### Swift Changes

- Queue view (upcoming tracks list)
- Next/previous buttons in transport bar
- Shuffle/repeat toggles
- Album context menu: "Play Album", "Queue Album"
- Gapless indicator in audio chain status

### Deliverable
Play an album gaplessly. Queue management. Shuffle/repeat.

---

## Phase 4: Format String Engine

**Goal:** fb2k-compatible format strings for views, columns, and file operations.

### Rust: `koan-core/src/format/`

#### `parser.rs` — Tokenizer
```rust
pub enum Token {
    Literal(String),          // plain text
    Field(String),            // %field%
    Conditional(Vec<Token>),  // [...]
    Function {                // $func(args)
        name: String,
        args: Vec<Vec<Token>>,
    },
}
pub fn parse(input: &str) -> Result<Vec<Token>, FormatError>;
```

#### `eval.rs` — Evaluator
```rust
pub trait MetadataProvider {
    fn get_field(&self, name: &str) -> Option<String>;
}
pub fn evaluate(tokens: &[Token], provider: &dyn MetadataProvider) -> String;
```
- `%field%` → lookup from provider, empty string if missing
- `[...]` → only output if ALL field lookups inside succeeded
- `'...'` → literal, no expansion

#### `functions.rs` — Built-in functions
- String: `$left`, `$right`, `$pad`, `$pad_right`, `$replace`, `$trim`, `$lower`, `$upper`, `$caps`
- Logic: `$if`, `$if2`, `$if3`, `$stricmp`
- Numeric: `$num`, `$div`, `$mod`
- Path: `$directory`, `$directory_path`, `$ext`, `$filename`
- Meta: `$info`, `$date`

### Integration Points

- Library tree views use format strings for grouping
- Playlist columns use format strings for display
- File rename/move uses format strings for path generation
- All configurable by user

### Deliverable
Custom library views and playlist columns driven by format strings.

---

## Phase 5: Full UI Polish

**Goal:** The spec's UI vision — minimal, dark, typographic, information-dense.

### Layout Implementation
- Transport bar: play/pause, prev/next, seek bar, time, volume
- Split view: library browser (left) + track list (right)
- Album art panel (bottom-left, large)
- Now Playing bar with audio chain status
- Resizable panes, remembers layout

### Aesthetic
- System dark mode, respects macOS appearance
- SF Pro for UI, SF Mono for technical info
- Album art as colour accent, UI is monochrome around it
- Subtle crossfade between album art on track change
- Dense but not cramped

### Album Art
- Read embedded art (lofty handles FLAC/MP3/M4A)
- Fallback: cover.jpg/cover.png/folder.jpg in album directory
- Thumbnail cache in SQLite (blob column)
- Grid view for album browsing
- High-res display in now-playing panel

### Keyboard Navigation
- j/k: up/down in lists
- Space: play/pause
- Enter: play selected
- /: search focus
- g/G: top/bottom
- ⌘K: command palette

### Media Keys
- MediaRemote framework for media key capture
- MPNowPlayingInfoCenter for Control Center/Lock Screen
- AirPlay device selection

### Deliverable
The full UI as specced. Looks good, feels native, keyboard-driven.

---

## Phase 6: Advanced Audio

**Goal:** ReplayGain, exclusive mode, integer mode, multi-format excellence.

### ReplayGain
- Read existing RG tags from lofty (track + album gain)
- Album mode by default
- Apply gain in 64-bit float before final output
- Limiter to prevent clipping (simple lookahead limiter)
- Scan & write RG tags: Symphonia decode → ebur128 → lofty write

### Exclusive/Integer Mode
- Hog mode toggle in preferences
- When enabled: take exclusive device access, set physical format
- Integer mode: set 24-bit physical format for compatible DACs
- Automatic sample rate switching based on source file
- Status shown in audio chain bar

### Software Volume
- Off by default (pure digital passthrough)
- When enabled: 64-bit float volume scaling before output
- Volume curve: logarithmic (perceptual)

### Deliverable
Audiophile-grade output. Bit-perfect chain visible in UI.

---

## Phase 7: File Operations

**Goal:** Rename/move files using format strings. Preview, batch, undo.

### Implementation
- Format string → path generation using metadata
- Preview panel: old path → new path for all affected files
- Move ancillary files (cover.jpg, .cue, .log) with music
- Remove empty directories after move
- Move log stored in SQLite for undo
- Named presets for common operations

### Deliverable
Batch rename/organize music library using format strings with full preview and undo.

---

## Phase 8: Subsonic/Navidrome (v2)

**Goal:** Stream from Navidrome, unified library view.

### Implementation
- Subsonic API client (REST, JSON responses)
- Stream original quality on LAN, configurable transcode for remote
- Offline cache: download pinned albums to local storage
- Hybrid library: local + remote unified, visual indicator
- Playlist sync, star sync, play count sync
- Scrobble plays back to server

### Deliverable
Seamless local + streaming library.

---

## Deferred to v2+

- DSP chain (EQ, room correction, crossfeed)
- Spectral analysis / fake lossless detection
- MusicBrainz auto-tagging / acoustid
- Smart playlists (query-based)
- Waveform display / spectrum analyzer
- Multiple output devices
- iOS companion
- Last.fm / Listenbrainz scrobbling
- CUE sheet support
- DSD native playback (DoP passthrough is Phase 6 stretch)

---

## Agent Execution Strategy

This section is for Claude Code to use when implementing each phase. Each phase should be broken into parallelizable work units.

### General Rules

- **Always wrap shell commands:** `zsh -i -c '...'` for mise/tool access
- **Always run formatter** after any code changes (rustfmt for Rust, swift-format for Swift if configured)
- **Always run `cargo clippy`** — zero warnings policy
- **Always run `cargo test`** after Rust changes
- **Parallelise aggressively** — Rust crates are independent, spin up agents for each

### Phase 0 Agent Plan

Launch **3 agents in parallel:**

1. **Agent: Rust workspace scaffold**
   ```
   Task(subagent_type="Bash", prompt="
     Set up Cargo workspace at /Users/james.cleveland/workspace/radiosilence/koan:
     - Cargo.toml workspace with members: crates/koan-core, crates/koan-ffi, crates/koan-audio-ffi
     - koan-core: lib crate, empty src/lib.rs with module stubs
     - koan-ffi: lib crate, depends on koan-core + uniffi, src/lib.rs exports version()
     - koan-audio-ffi: staticlib crate, depends on koan-core, src/lib.rs exports extern C koan_ping
     - Add all dependencies from the crate stack to appropriate Cargo.tomls
     - cbindgen.toml for koan-audio-ffi
     - uniffi.toml for koan-ffi
     - Run cargo build to verify
   ")
   ```

2. **Agent: Swift package scaffold**
   ```
   Task(subagent_type="Bash", prompt="
     Set up Swift package at /Users/james.cleveland/workspace/radiosilence/koan/swift:
     - Package.swift with targets for Koan app
     - Stub KoanApp.swift with @main and a basic SwiftUI window
     - Stub ContentView.swift showing 'kōan' text
     - NOTE: Don't try to link Rust yet, just get Swift compiling standalone
   ")
   ```

3. **Agent: Build scripts + Makefile**
   ```
   Task(subagent_type="Bash", prompt="
     Create build system at /Users/james.cleveland/workspace/radiosilence/koan:
     - Makefile with targets: build-rust, generate-bindings, build-xcframework, build-swift, run, clean, dev
     - scripts/build-rust.sh: cargo build --release for both FFI crates
     - scripts/generate-bindings.sh: run uniffi-bindgen-swift + cbindgen
     - scripts/build-xcframework.sh: combine static libs + headers + Swift bindings into xcframework
     - scripts/build.sh: orchestrate all of the above
     - .gitignore for target/, .build/, *.xcframework, generated bindings
   ")
   ```

**Then sequentially:** Wire them together — update Package.swift to reference xcframework, verify `make run` works end-to-end.

### Phase 1 Agent Plan

Launch **3 agents in parallel:**

1. **Agent: CoreAudio wrapper** (`koan-core/src/audio/`)
   ```
   Task(prompt="
     Implement CoreAudio output engine using objc2-core-audio:
     - device.rs: enumerate devices, get/set sample rate, hog mode
     - engine.rs: AUHAL AudioUnit, render callback draining rtrb consumer
     - format.rs: physical format switching, integer mode
     Context: The render callback is real-time — no allocations, no locks, only atomics.
     Use objc2-core-audio for HAL property access. Reference MPV's ao_coreaudio.c for
     the hog mode / integer mode / sample rate switching patterns.
   ")
   ```

2. **Agent: Decode pipeline** (`koan-core/src/audio/buffer.rs` + `player/`)
   ```
   Task(prompt="
     Implement Symphonia decode → rtrb buffer pipeline:
     - buffer.rs: spawn decode thread, open file via Symphonia, decode to f32, push to rtrb
     - player/state.rs: PlaybackState enum, NowPlaying struct with atomics
     - player/commands.rs: PlayerCommand enum, lock-free SPSC command channel
     - Wire it together: command → open file → decode → buffer → ready for engine
   ")
   ```

3. **Agent: C FFI + Swift bridge** (`koan-audio-ffi/` + Swift)
   ```
   Task(prompt="
     Implement the C FFI layer and Swift audio bridge:
     - koan-audio-ffi/src/lib.rs: extern C functions (init, play, pause, resume, stop, seek, get_state, get_position, get_now_playing)
     - Global static engine instance (OnceLock<AudioEngine>)
     - Swift AudioBridge.swift: wraps C functions in Swift-friendly API
     - Swift TransportBar.swift: play/pause, seek slider, position display
     - Swift file open dialog → pass path to koan_play()
   ")
   ```

### Phase 2 Agent Plan

Launch **3 agents in parallel:**

1. **Agent: Database** (`koan-core/src/db/`)
   ```
   Task(prompt="
     Implement SQLite database layer:
     - schema.rs: all tables from the schema design, migration system
     - connection.rs: WAL mode, reader/writer separation, prepared statement cache
     - queries.rs: insert/upsert for artists/albums/tracks, search via FTS5,
       get_artists, get_albums, get_tracks, library stats
     - Comprehensive tests for all queries
   ")
   ```

2. **Agent: Scanner + metadata** (`koan-core/src/index/`)
   ```
   Task(prompt="
     Implement library scanner:
     - scanner.rs: parallel directory walk (rayon), filter audio extensions,
       check scan_cache, extract metadata, batch insert
     - metadata.rs: lofty wrapper for unified tag reading (title, artist, album,
       disc/track, date, genre, label, MBID, duration, format info, embedded art, RG tags)
     - watcher.rs: notify FSEvents, 2s debounce, handle create/modify/delete/rename
     - Target: 50k tracks in <10s
   ")
   ```

3. **Agent: Library UI** (Swift)
   ```
   Task(prompt="
     Implement SwiftUI library browser:
     - LibraryViewModel.swift: wraps UniFFI library functions
     - LibraryBrowser.swift: 3-pane artist → album → track browser
     - TrackList.swift: table with columns (#, Title, Artist, Album, Duration, Format)
     - SearchViewModel.swift + search bar with live FTS5 results
     - Settings view: add/remove library folders, trigger scan
     - Double-click track → play via AudioBridge
   ")
   ```

### Phase 3 Agent Plan

Launch **2 agents in parallel:**

1. **Agent: Gapless engine** (Rust)
   ```
   Task(prompt="
     Implement gapless playback:
     - Dual ring buffer with atomic swap
     - Pre-decode next track when current reaches last 5 seconds
     - Read LAME gapless headers for MP3
     - Trim encoder delay/padding at boundaries
     - Queue management: PlayQueue struct, next/prev, shuffle, repeat
     - Extended PlayerCommand variants
   ")
   ```

2. **Agent: Queue UI** (Swift)
   ```
   Task(prompt="
     Implement queue UI in SwiftUI:
     - Queue view showing upcoming tracks
     - Next/previous buttons in transport
     - Shuffle/repeat toggles
     - Context menus: Play Album, Queue Album, Play Artist
     - Drag to reorder queue
   ")
   ```

### Phase 4 Agent Plan

**Single agent** — the format string engine is a self-contained Rust module:

```
Task(prompt="
  Implement fb2k-compatible format string engine:
  - parser.rs: tokenize format strings into AST (Field, Literal, Conditional, Function)
  - eval.rs: evaluate AST against a MetadataProvider trait
  - functions.rs: all built-in functions ($if, $left, $right, $pad, $num, $replace, etc.)
  - Comprehensive test suite covering all format string features
  - Integration: MetadataProvider impl for TrackInfo (from DB) and for lofty tags (from file)
  This is a critical module — the format string spec from fb2k must be followed precisely.
  Test edge cases: nested conditionals, missing fields, function chaining.
")
```

### Phase 5 Agent Plan

Launch **3 agents in parallel:**

1. **Agent: UI layout + theming** (Swift)
   ```
   Task(prompt="
     Implement the full UI layout from spec:
     - Transport bar with all controls
     - Split view: library browser (left) + track list (right), resizable
     - Album art panel (bottom-left)
     - Now Playing bar with audio chain status
     - Dark mode, SF Pro/SF Mono typography
     - Layout persistence (remember pane sizes)
   ")
   ```

2. **Agent: Album art pipeline** (Rust + Swift)
   ```
   Task(prompt="
     Implement album art:
     - Rust: extract embedded art via lofty, search for cover.jpg/png/folder.jpg
     - Rust: generate thumbnails, cache in SQLite blob column
     - FFI: expose art retrieval (thumbnail for grid, full-res for now-playing)
     - Swift: AlbumArtView with crossfade animation on track change
     - Swift: Album grid view for browsing
   ")
   ```

3. **Agent: Keyboard + media keys** (Swift)
   ```
   Task(prompt="
     Implement keyboard navigation and media keys:
     - Vim-style: j/k navigation, /, g/G, Space play/pause, Enter play
     - Command palette (⌘K) with fuzzy search
     - MediaRemote framework for media keys
     - MPNowPlayingInfoCenter for Control Center integration
     - Customizable keybindings (stored in UserDefaults or JSON config)
   ")
   ```

### Phase 6 Agent Plan

Launch **2 agents in parallel:**

1. **Agent: ReplayGain** (Rust)
   ```
   Task(prompt="
     Implement ReplayGain:
     - Read existing RG tags (track + album gain) from lofty
     - Album mode by default, track mode option
     - Apply gain in 64-bit float in the audio pipeline (after decode, before output)
     - Simple lookahead limiter to prevent clipping
     - Scan command: decode via Symphonia → feed to ebur128 → compute gain → write via lofty
     - R128/EBU loudness normalization as alternative mode
   ")
   ```

2. **Agent: Exclusive/Integer mode** (Rust)
   ```
   Task(prompt="
     Implement exclusive audio mode:
     - Preferences: exclusive mode toggle, integer mode toggle
     - When enabled: hog device, set physical format, switch sample rate
     - Automatic sample rate switching: detect source rate, switch device to match
     - Integer mode: 24-bit physical format for compatible DACs
     - Audio chain status: show full signal path in FFI-exposed struct
     - Handle device changes gracefully (unplug/replug)
   ")
   ```

### Testing Strategy

Every phase should include:
- **Rust unit tests** for all core logic (`cargo test`)
- **Rust integration tests** for FFI boundary
- **Manual testing checklist** per phase (documented in phase deliverable)
- **clippy clean** — `cargo clippy -- -D warnings`
- **No compiler warnings** in either Rust or Swift

### Build Verification (run after every phase)

```bash
# Full build + test cycle
zsh -i -c 'cd /Users/james.cleveland/workspace/radiosilence/koan && make clean && make build && cargo test --workspace && cargo clippy --workspace -- -D warnings'
```

---

## Distribution

- **Development:** `make run` builds and launches
- **Release:** `make release` → builds optimized, signs, notarizes
- **Homebrew:** Cask formula pointing to GitHub releases (.dmg or .zip)
- **Auto-update:** Sparkle framework for in-app updates
- **Signing:** `rcodesign` (pure Rust) or `codesign` if Xcode available

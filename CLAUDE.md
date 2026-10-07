# Project Rules

## What is koan

Bit-perfect music player (macOS + Linux). Rust core, Ratatui TUI, plus a native SwiftUI app on macOS. Five crates:

- **koan-core** — library crate. Audio engine, player, database, indexer, format strings, file organization, remote (Subsonic/Navidrome) client, shared helpers. No UI code, no terminal deps.
- **koan-tui** — library crate. Ratatui TUI, visualizers, media keys. Exports `run_tui()`. Depends on koan-core.
- **koan-server** — library crate. GraphQL (async-graphql + axum), Subsonic REST API, MCP server. Depends on koan-core.
- **koan-ffi** — staticlib/cdylib crate. uniffi bindings exposing koan-core to Swift. Depends on koan-core only. Not published to crates.io.
- **koan-cli** — binary crate (`koan`). Thin entry point: clap CLI, logger, signal handling, command routing. Depends on koan-core + koan-tui + koan-server.

Plus **tools/uniffi-bindgen** — the Swift bindings generator (`just ffi-bindings`). A crate of its own so that running it builds uniffi and nothing else; it reads the bindings from any built koan-ffi library, the iOS one included.

Plus **apps/macos** — SwiftUI app (SwiftPM, Swift 6, macOS 26+). Links koan-ffi.

Dependency rules (compiler-enforced): koan-tui, koan-server and koan-ffi cannot import each other; all three depend only on koan-core. Native clients import koan-core through koan-ffi.

**Local UI goes through FFI, not GraphQL.** The macOS app links the engine in-process — a daemon, a port and an auth surface buy nothing when the UI is sitting on top of the audio engine. GraphQL is the surface for clients that genuinely can't link the core: the web UI, jukebox remotes. The iOS app links the core like the Mac app does. When adding a capability to one, consider whether the other needs it too — both are thin shims over the same koan-core helpers.

## Architecture overview

Read `ARCHITECTURE.md` for the full technical manual (threading model, data flow, sync primitives, module reference). This section is the quick-ref.

### Threading model (5 threads at steady state)

```
Main Thread (TUI, 60fps)   ──crossbeam channel──►  Player Thread ("koan-player")
                                                       │
                                                       ├──rtrb ring buffer──►  Decode Thread ("koan-decode")
                                                       │
                                                       └──controls──►  Audio RT Thread (CoreAudio/cpal, system-managed)

Analyzer Thread ("viz-analyzer") ◄──VizBuffer──  Decode Thread
                                  ──VizSnapshot──►  Main Thread (TUI)
```

**Golden rule: the audio render callback must NEVER allocate or lock.** It only touches atomics and the rtrb consumer.

### Sync primitives

| Data | Primitive | Why |
|------|-----------|-----|
| PCM samples (decode→audio output) | `rtrb` SPSC ring buffer | Lock-free, cache-friendly |
| Commands (TUI→Player) | `crossbeam-channel` bounded(16) | Backpressure, timeout recv |
| Atomics (position, state, samples_played) | `AtomicU8/U64/Bool` Relaxed | Hot path, no contention |
| Complex shared state (playlist, track info) | `parking_lot::RwLock` | Faster than std, no poisoning |
| Viz samples (decode→analyzer) | `VizBuffer` (`parking_lot::Mutex`) | Ring of f32 for FFT |
| Analysis output (analyzer→TUI) | `VizSnapshot` (`parking_lot::Mutex`) | Atomic snapshot |
| Parallel work (scan, remote sync) | `rayon` | Work-stealing thread pool |

### Key data flow

```
File → Symphonia → f32 → rtrb ring buffer → platform audio callback → DAC
```

No resampling. Device sample rate switched to match source (bit-perfect). Float32 all the way.

### Key design decisions

- **QueueItemId (UUIDv7)** — all queue ops use IDs, not indices. Survives reordering, handles duplicate tracks.
- **Status is derived** — `QueueEntryStatus` computed from cursor + load state, never stored.
- **Decode cursor ≠ UI cursor** — decode thread peeks ahead for gapless without moving the playlist cursor.
- **One `derive_visible_queue()` per frame** — cached snapshot, all render/mouse ops see consistent state.
- **Tracks are derived from sources** — each file is a `local_files` row and each server entry a `remote_entries` row, holding its own tags. A track holds at most one of each; its columns are derived, the file's first. `sources::link` alone decides which track a source is (MusicBrainz recording + release, or album/album artist/disc/number/title with the artist as tie-break; ambiguous → declined). Never write track identity columns outside `db/queries/sources.rs`. Albums are title + album artist + release (editions are separate albums); artists are their folded name; favourites reference rows by id.
- **Uids, not row ids, leave the database** — every artist, album, track and playlist has a UUIDv7 `uid`, published by Subsonic, GraphQL and the link. Clients syncing from a koan server adopt its uids, so ids mean the same thing on every device. See `db/queries/uids.rs`.
- **Figment-layered config** — defaults → `config.toml` → `config.local.toml` → `KOAN_*` env vars. All writes go through `Config::persist()`, which diffs the mutation and routes each changed key by `config::layer_of` — secrets, this machine's paths/hardware/account and volatile UI state to `config.local.toml`, taste to `config.toml`. Comments survive; untouched keys are never rewritten.

## Git

- **NEVER push tags.** Tags and releases are handled externally. Only push commits.
- Work in PRs, never push to main.
- Don't rebase on merge — we squash PRs.

## Docs and site

`docs/` is the user guide and reference, and koan.rocks is built from it (`site/src/docs.sh`, deployed by `site.yml`). A change in behaviour, a setting, a CLI flag or a supported platform updates the matching page in the same PR, as does a fragment under `changelog.d/unreleased/<section>/` (see `changelog.d/README.md`). CHANGELOG.md is generated by `scripts/changelog.py`; never edit it by hand. Before a release, check that the guide still describes what the code does, since screenshots and settings drift fastest.

## Supervising sessions

If you are told you are the foreman, supervisor, manager, boss or guvnor of the koan sessions, in any wording, this section is your brief: read it before doing anything else. Other sessions skip it.

The manager coordinates and does not write features. It keeps the context, makes the decisions and hands each piece of work to another session as a self-contained task: the exact branch, the exact change, the command that verifies it, and what to report back.

- **Orient first.** Run `date`, then `gh pr list`, `gh issue list`, `git worktree list`, `gh release list --limit 3` and `ListAgents`. The open PRs and their CI state are the work in flight. Main's last `release: vX.Y.Z` commit tells you what has shipped since.
- **Workers.** Sessions named `worker-<name>` each run in `../worktrees/koan/worker-<name>` and switch to whichever PR branch they are given. Other named sessions own their own area, such as the Apple TV app, and get status requests and briefs rather than unrelated tasks. A session that has hit its context limit is cleared by the user and then re-briefed by the manager with everything it needs in one message. A compacted session loses detail, so prefer clearing.
- **Merging.** The manager merges green PRs when the user asks it to (`gh pr merge --squash`). When PRs conflict, merge main into the branch and push. The manager's token cannot merge a PR that changes `.github/workflows/`, so the user merges those.
- **Releasing.** Cut a release once the in-flight PRs the user wants are on main, using the `release` skill. Merging the release PR is what publishes it: CI releases from main when the version in `Cargo.toml` moves. When the release is published, trigger the server deploy by hand and confirm the TestFlight builds uploaded. The release PR runs `just changelog --release X.Y.Z`, which turns the unreleased fragments into that version and regenerates CHANGELOG.md; edit fragments, not the generated file, to change the notes.
- **Docs.** Before cutting a release, have a session check `docs/` and the site against what merged since the last one.
- **Housekeeping.** Remove a PR's worktree and its `target/` once it merges or is abandoned; Cargo target directories run to gigabytes each. Keep at most one booted simulator across all sessions, never drive or capture the user's live screen, and stop only the processes you started.
- **Secrets.** Signing and App Store Connect credentials come from the 1Password service account (`op`). Export a key to the scratchpad only for the command that needs it, and delete it afterwards. Never write a credential into the repo, a PR or an issue.

## Build & check

```bash
just check          # cargo test + clippy -D warnings
just fmt            # cargo fmt
just theme-leaks    # styling in the apps that bypasses the kōan theme's roles
just cli            # cargo run --release -p koan-cli -- <args>
just build          # cargo build --release
just macos-run      # build + launch the macOS app
just macos-dmg      # package the app for release
just css            # compile the web UI's Tailwind stylesheets (output is committed)
just ios-typecheck  # the shared SwiftUI sources still build for iOS
just ios-run        # build and launch on a booted simulator
just ios-smoke FILE # play a file through the real Player on the simulator
just ios-phone      # install on the plugged-in iPhone (personal team)
just ios-walk       # UI test that screenshots every page, into target/ios-walk
just ios-bars       # every scrolling page clears the kōan bar, on an iPhone and an iPad, against a generated library
just ios-testflight BUILD # archive, sign and upload to TestFlight (needs the ASC key)
just ios-signin DEV URL USER PASS # sign a simulator in through Settings, as App Review does
just ios-use DEV      # use the app like a listener and check each step; run before submitting
just ios-store-shots SRC OUT [captions-ipad] # frame a walk's screenshots for the App Store
just ios-store [iphone=DIR ipad=DIR] # push apps/ios/store/listing.toml (+ screenshots) to App Store Connect
just tv-run         # build and launch on an Apple TV simulator
just tv-device      # install on the Apple TV paired with Xcode
just tv-walk [LIBRARY] # walk every page with the remote, into target/tv-walk; a folder is served from a throwaway koan
just tv-join LINK [tv] # sign the simulator, or the paired Apple TV, in through an invite
just tv-kept        # clear Caches/koan-config as tvOS does, check the TV is still signed in
just tv-pair [OUTCOME] # pair a signed-out TV simulator, approved (or declined, or left to expire) over the API
just tv-signin [ROUTES] # each other way onto the TV and its failures, from fresh installs, against KOAN_SIGNIN_*
just tv-pair-qr     # the same, approved by an iPhone simulator that reads the QR code off the TV
just tv-discover    # a signed-out TV simulator finds a server the Mac announces as a signed-in device would, and pairs
just tv-testflight BUILD # archive, sign and upload the tvOS app to TestFlight (needs the ASC key)
```

The macOS app needs `just macos-ffi` to have run at least once — it generates the Swift bindings that `swift build` compiles against. `macos-build` does this for you.

Development builds are signed with a self-signed certificate — `just macos-signing-cert` creates it, once. Without it the app is ad-hoc signed, which derives its identity from the binary's own hash: every rebuild is a different application to macOS, so TCC permissions are forgotten each time. It buys nothing against Gatekeeper, which wants Developer ID and notarisation.

Pre-push hook (`.claude/settings.json`) runs `cargo fmt --all` + `cargo clippy --workspace -- -D warnings` before any `git push`. If clippy fails, fix before pushing.

`deploy/pulumi/package.json`'s `version` and `deploy/pulumi/src/versions.ts`'s `APP_VERSION` move with the workspace version in `Cargo.toml`; `check-version` in CI fails a build where they drift.

**Zero warnings policy.** Fix all clippy/compiler/lint warnings immediately. Run fmt after every change.

## Where things live

### koan-core (`crates/koan-core/src/`)

| Module | What |
|--------|------|
| `audio/backend.rs` | `AudioBackend` + `AudioEngineHandle` traits — platform-agnostic audio output |
| `audio/coreaudio_backend.rs` | macOS `CoreAudioBackend` impl (wraps engine.rs + device.rs) |
| `audio/ios_backend.rs` | iOS and tvOS `IosAudioBackend` impl — the route is the only device; the session belongs to the app |
| `audio/toolbox.rs` | The AudioToolbox names the engine uses, declared by hand for tvOS: `coreaudio-sys`'s build script knows only macOS and iOS |
| `audio/cpal_backend.rs` | Linux `CpalBackend` impl (ALSA/PipeWire/PulseAudio via cpal) |
| `audio/engine.rs` | CoreAudio output setup, render callback. AUHAL on macOS, RemoteIO on iOS — two properties apart |
| `audio/buffer.rs` | `PlaybackTimeline`, track boundaries, decode thread entry points (`start_decode`, `decode_queue_loop`, `decode_single`) |
| `audio/device.rs` | CoreAudio device enumeration, sample rate get/set (macOS only) |
| `audio/replaygain.rs` | EBU R128 loudness scanning, gain application via lofty |
| `audio/dsp/` | EQ and convolution per output device, on the decode thread. No profile, no chain: bit-perfect stays checkable. Delays trimmed and flushed so the timeline counts output time. `steps.rs` runs a profile's filters in order — bands, delays, mixes, graphic curves as minimum-phase FIRs — since a mix does not commute; `impulse.rs` is the routing matrix (Convolver's model; a WAV is its diagonal); `import.rs` sorts files, folders and zips into one profile through `convolver`, `camilla`, `apo` and `raw`; `profiles.rs` is what every front end calls |
| `audio/viz.rs` | `VizBuffer` (ring of f32 samples for analyzer), `VizSnapshot` (atomic snapshot for UI), `VizLevels` (the spectrum as three bands, for callers that poll often and draw little) |
| `audio/analyzer.rs` | FFT analysis thread — 48-band spectrum, VU meters, peak hold. Runs at configurable FPS |
| `audio/streaming.rs` | `PartialFileSource` — reads a download in progress off disk, blocking at the write head |
| `player/mod.rs` | `Player` struct, command loop (`run()`), `start_playback()`, `update_playback_state()` |
| `player/commands.rs` | `PlayerCommand` enum, `CommandChannel` (bounded crossbeam) |
| `player/state.rs` | `SharedPlayerState`, `Playlist`, `PlaylistItem`, `QueueItemId`, `LoadState`, `PlaybackState`, `derive_visible_queue()` |
| `player/undo.rs` | Undo/redo stack for playlist operations (100-deep) |
| `player/renderer.rs` | A UPnP renderer as a session's output: `RendererLink` (the chosen renderer, kept across sessions) and `Output::Renderer`, which hands over the original file (gapless via `SetNextAVTransportURI`) or, with a profile for the renderer, a stream processed here. The playhead is the renderer's clock. Renderer events return as session-tagged `PlayerCommand::Renderer` |
| `player/history.rs` | Play history recording — writes an entry when a track starts, fills in listening time when it ends. Owns the `koan-history` writer thread |
| `db/schema.rs` | DDL: artists, albums, tracks, scan_cache, remote_servers, organize_log, tracks_fts (FTS5) |
| `db/connection.rs` | `Database::open()`, WAL mode, pragmas |
| `db/pool.rs` | Connections opened once and kept. What every front end reads through — `Database::open` checks the schema and checkpoints the WAL, which is not a thing to do per query |
| `db/queries/` | Row types, upsert, `sources` (track identity: source rows, link, derive), FTS5 search, scan cache, stats, playlists, `smart` (smart playlist rules compiled to SQL, evaluated on read into the playlist's entries), `batch` (SQL-side track filtering, batched parent→child reads) |
| `index/scanner.rs` | Streaming library scan: walkdir → rayon tag reads → bounded channel → batched DB transactions. `ScanOptions` carries a cancel flag and an optional progress sink. `import_paths` indexes named files where they lie (Finder drops), removing nothing; `scan_dirs` rescans named directories inside the library, removals included — what the folder watcher runs |
| `index/playlist_files.rs` | Playlist files found by scans, keyed by `source_path`: Navidrome `.nsp` into smart playlists, `.m3u`/`.m3u8` into read-only ordinary ones. A file gone from a settled directory deletes its playlist |
| `index/lane.rs` | One scan at a time: every scanner entry point (full scan, folder and directory rescans) waits for the lane; an import does not, and waits only for a folder being forgotten. The scan in it can be cancelled by the person (`cancel_all`, which the apps' Cancel calls) or by forgetting a folder it covers (`cancel_under`) |
| `index/watch.rs` | Which filesystem events can change the index, and the directory each one means a scan of. Drops access, metadata, hidden and Syncthing paths, partial downloads |
| `index/metadata.rs` | Tag reading via lofty (ID3, Vorbis, MP4, APE), codec detection |
| `index/folder_art.rs` | Covers as image files beside the tracks (`cover.*`, `folder.*`, `front.*`, Navidrome's order), ahead of embedded art. `cover_art` is what every cover lookup calls. Directory listings kept against the directory's mtime |
| `index/id3v2_pictures.rs` | MP3 tag reads with the embedded art held back — walks the ID3v2 frame headers and serves lofty zeros over the picture frames it would only discard |
| `format` | fb2k-compatible template engine, re-exported from sift (`sift-music`) — the tagger shared with other importers. Change it there |
| `remote/client.rs` | Subsonic/Navidrome HTTP client (reqwest blocking, MD5+salt auth) |
| `remote/download.rs` | Streaming downloads: `.part` → verify → atomic rename, progress, retries. All disk-bound remote bytes go through here |
| `remote/sync.rs` | Library sync: album list, then songs paged in bulk via empty-query `search3` (per-album `getAlbum` for servers that cannot), one transaction per page, progress per page. Every sync walks everything; `helpers::sync_remote` decides whether to walk, by the server's `getIndexes` `lastModified` |
| `remote/history.rs` | Play history shared through a koan server: the outbox of scrobbles and forgettings (offline plays reach the server dated to when they started), and adopting the account's other devices' plays after a cursor. Gated on `koanHistory` |
| `remote/link.rs` | The standing WebSocket a client keeps to a koan server (`/rest/koanLink`), and the `LinkCommand`s the server sends down it: play, enqueue, pause, skip. Reconnects on its own; ids are resolved to local tracks, syncing first if one is new |
| `remote/pair.rs` | A device without a keyboard signing in: opens `/rest/koanPair`, shows the code and `koan.rocks/pair/` link, and blocks until the server sends the outcome; an approved key is stored as an invite's is. Also the approver's calls (`info`, `approve`, `decline`) and `PairLink` |
| `remote/profile.rs` | What the signed-in server is: `ping` + `getOpenSubsonicExtensions`, once per sign-in. Gate koan features on the extension (`koanLink`, `koanDevices`), never on the server's name |
| `remote/scrobbling.rs` | The account's scrobbling on its koan server (`koanScrobbling`): status, connect with a ListenBrainz token, disconnect. The server keeps the token and never returns it |
| `remote/connections.rs` | Who is connected to this device and whom it is connected to — the server link, the account's devices behind it, nearby sessions either way — for Settings to list and end |
| `remote/devices.rs` | The devices this one can play on — the account's from the link, the network's from `nearby` — which one the app controls, and getting a command to it |
| `remote/nearby.rs` | LAN control: listener on `devices.port`, Bonjour via `dns_sd`, a connection per device found or listed by address. Strangers get playback and the queue only (`LinkCommand::allowed_nearby`) |
| `remote/wire.rs` | Event-driven WebSocket sessions: one `poll` on the socket and a pipe the engine's change signal rings |
| `remote/wikimedia.rs` | Wikidata items, Wikipedia lead sections and Commons images — where artist bios and photos come from |
| `remote/queue.rs` | The download queue: what to fetch when. Follows the playlist (started by `Player::spawn`; front ends never enqueue), worker pool, a priority lane for the track under the cursor, cursor-aware reordering. With a cache limit, fetches only as far ahead of the cursor as the limit allows (the playback window), and trims the cache |
| `remote/downloads.rs` | The download store, owned by `SharedPlayerState`: the one table of transfers, keyed by track, with every queue entry waiting on each. `settle` is the one way a transfer's end is told |
| `upnp/` | Playing to UPnP/DLNA MediaRenderers. SSDP discovery, SOAP, GENA, DIDL-Lite, the tokenised HTTP listener renderers fetch from, and `stream.rs`, which encodes a processed session for one. Hand-rolled on std threads; an event is a reason to ask the renderer where it is, never a reading |
| `quiet.rs` | What runs in the background on iOS: nothing nobody asked for. Link, nearby browse and dial, sync and rescans wait here; a phone playing stays findable. Lifted by controlling another device or a push |
| `config.rs` | Figment-based layered config: defaults → config.toml → config.local.toml → KOAN_* env vars |
| `helpers.rs` | Shared by every front end: sign-in, favourite reconciliation, sharing, auto-sync and folder watching, forget-folder/forget-remote, cache and index maintenance |
| `playlists.rs` | Playlists beyond the database: two-way Subsonic reconciliation, background pushes, M3U8 export. Read-only playlists (smart ones here, `readonly` ones there) are never pushed |
| `smart.rs` | Smart playlist rules: the typed model and its JSON, checked with errors that name the problem, and Navidrome `.nsp` parsing into it |
| `shelves.rs` | Favourites, Recently played and a search as filters on the album, artist and track listings (`AlbumQuery`, `ArtistQuery`, `TrackFilter`). A shelf page's previews are the head of those listings, with counts from the same query, so a preview and its "See all" agree. What each shelf holds, its window and its order live here; front ends ask by name |
| `organize.rs` | File rename using format strings. Preview/execute/undo — one `PlanEntry` per file carrying its destination and outcome. Moves ancillary files |
| `lyrics.rs` | LRCLIB lyrics fetching and parsing (synced LRC + plain) |
| `scrobbling.rs` | Forwarding plays to ListenBrainz: one sleeping thread, woken when a play is queued, sending the durable `scrobble_outbox` in batches and backing off only while the service is unreachable. Now-playing notices are best effort |
| `artist_info.rs` | Artist bio and photo: MusicBrainz id → Wikidata → Wikipedia/Commons. Resolved by id, never by name alone; cached per artist, misses included |

### koan-tui (`crates/koan-tui/src/`)

| Module | What |
|--------|------|
| `play.rs` | `run_tui()` — TUI event loop entry point, frame timing, input handling |
| `app.rs` | `App` state machine, `Mode` enum, event handlers per mode |
| `ui.rs` | Render pipeline: layout → transport → content → overlays → hints |
| `transport.rs` | Transport bar widget: seek bar, track info, click-to-seek |
| `queue.rs` | Album-grouped queue with status icons, selection, drag targets |
| `library.rs` | Flattened tree (artist→album→track), expand/collapse, substring filter |
| `picker.rs` | Nucleo fuzzy search, multi-select, colored matches |
| `cover_art.rs` | Halfblock rendering (2px per terminal cell, Lanczos3 resize) |
| `visualizer.rs` | Spectrum analyzer widget (reads `VizSnapshot`) |
| `lyrics.rs` | Lyrics side panel — synced line highlighting, scroll |
| `organize.rs` | Organize modal: pattern picker → preview table → background execute |
| `media_keys.rs` | macOS Control Center via souvlaki, manual CFRunLoop pump |
| `enqueue.rs` | `enqueue_playlist()` — build PlaylistItems from track IDs, submit downloads |
| `remote_bridge.rs` | Remote control: mirrors a koan server's now-playing and queue into the TUI over GraphQL and sends its commands there. No local playback |

### koan-ffi (`crates/koan-ffi/src/`)

| Module | What |
|--------|------|
| `lib.rs` | `KoanEngine` — the whole facade. Transport, queue ops, library queries, favourites, playlists, devices, scan. Every call that can block is `async`; only single-atomic reads stay sync |
| `offload.rs` | Where blocking work goes — a growing thread pool for reads, and one ordered lane for anything that ends in a `PlayerCommand` |
| `state.rs` | The engine's state as slices, and one cursor per client. Whole snapshots, batched at a tick, cut by rate of change |
| `queue_slice.rs` | The queue as clients are sent it: whole on an edit, and between edits a patch of the rows whose status moved |
| `types.rs` | uniffi records mirroring koan-core types (`Track`, `Album`, `NowPlaying`, `QueueItem`, …) and the conversions |

Swift bindings are generated, not checked in — `just macos-ffi` builds the lib and regenerates them.

### apps/macos (`apps/macos/Sources/`)

`Koan/` is the app: models, pages and rows, shared by every platform. `KoanIOS/`
is the iOS scene root and audio session — the phone's shell over the same state,
which tvOS uses too. `KoanTV/` holds what only a television has: Now Playing as
a page, share links as codes to scan. What tvOS lacks — hover, drag, the
pasteboard, sliders, keyboard shortcuts, selection menus — goes through small
shims in `Support/Platform.swift` that leave a view as it is there.
The directory is still called `macos` because the macOS app is what it builds
with SwiftPM. iOS device builds and the UI walk go through an Xcode project that
XcodeGen generates from `apps/ios/project.yml` (`just ios-project`); it is not
checked in.

On iOS each tab is a `NavigationStack` with its own path of `Route`s. Pages draw
from the route that pushed them, never from `nav.current`, and the navigator
follows the top of the stack in front — see `TabShell`.

| Module | What |
|--------|------|
| `KoanApp.swift` | `@main`, `AppState`, menu commands, keyboard shortcuts |
| `KoanIOS/PushDelegate.swift` | Push: registers for a token and sends it up the link; wakes to link on a background push; runs the command a tapped notification carries |
| `Support/ActivityModel.swift` | The one place that knows what koan is busy with. Each task declares what it holds — files on disk, local rows, remote rows, downloads — and a new one is disabled only where those overlap |
| `Support/Pairing.swift` | Approving a device that is waiting to sign in: a `koan.rocks/pair/` link arriving through `AppState.open(url:)`, or a code typed in Settings, asks the server for the device's name and then the person, as invites do |
| `Support/SettingsModel.swift` | Settings state over `config.toml`. Commits on edit, re-reads on focus |
| `Support/KoanTheme.swift` | The kōan theme: tokens, the record accent (OKLCH, cached per colour), motion, and the roles views use instead of colours, fonts, corners and materials. Each role draws the platform's look unchanged when the theme is off. Spec: `docs/design/koan-theme.md`; `just theme-leaks` finds what bypasses it |
| `Support/KoanThemeSheet.swift` | Every component of the theme on one page, for the evidence renderer |
| `Support/EngineMirror.swift` | The engine's state as SwiftUI sees it. `Observable` by hand: one property per slice, invalidated only where a slice actually moved |
| `Support/PlayerModel.swift` | What the app *does* to the player — commands, and the little that is genuinely local. Reads everything through the mirror |
| `Support/Navigator.swift` | Where the app is: one page, the linear history of pages visited, and a cursor. No `NavigationStack` — koan navigates like a browser, any page from any page |
| `Support/LibraryModel.swift` | Browse state. Holds what the section on screen is showing and nothing else — narrowing and sorting happen in SQL, listings arrive whole. Follows the navigator; never moves it |
| `Support/PlayableSelection.swift` | Things picked out of a page to play or queue together — the album grid, an artist's records, search results of every kind. A mode, because a click already plays or navigates. Held in tick order, so a pick can span several filters or queries |
| `Support/CoverArtCache.swift` | Album-keyed art cache: bytes once per record on disk, bitmaps per record and draw size in a bounded `NSCache`. Deliberately off the main actor — see the note there. Each miss is an HTTP round trip on remote libraries |
| `Support/ImageWork.swift` | The two lanes image work runs in, neither of them the cooperative pool: a wide one for blocking file reads, a bounded one for decoding |
| `Support/Platform.swift` | The few types AppKit and UIKit disagree about. `KoanApp`, `RootView`, `Hotkeys`, `TextFocus`, `EditCommands`, `MenuShortcuts` and `ShortcutsSheet` are the macOS shell and have no iOS counterpart; everything else builds for both |
| `Views/DriftingWash.swift` | The window's wash, as Core Animation. Drift, blur and dissolve belong to the compositor; nothing here costs a main-thread frame |
| `Support/FrameTimer.swift` | Times a tap against the display link, so the region after a body evaluation — layout, the commit, the render server — is measurable at all. See CONTRIBUTING |
| `Support/TransferMeter.swift` | Download progress at the display's rate: a display link, alive only while a transfer runs and something on screen draws it, reading byte counts per frame and handing them to rings and bars as layer geometry |
| `Support/PlayingLevels.swift` | One analyser subscription for every playing indicator on screen, handing each frame straight to the bars as layer geometry — nothing observable, nothing SwiftUI re-runs. Reads the stream only while a bar is attached, which is what lets the analyser park |
| `Views/QueueView.swift` | The main stage — album-grouped queue, drag reorder, multi-select. On the Mac a `KoanTable` of `QueueTableRow`s. Never torn down: `StageView` keeps it mounted behind other pages, so its place and its playing row survive a visit elsewhere. The album and artist browsers are rebuilt on each visit and restore their scroll position; kept mounted, they made every page switch lay them out |
| `Views/AlbumCollection.swift` | The Mac's album grid: `NSCollectionView`, tiles of layers and labels. SwiftUI's grid cost 22–27 ms per scroll step at 4K; this one 11–12. Behaviour mirrors `AlbumGridCell`, which iOS and the artist page keep. AppKit controls are SwiftUI graphs on macOS 26, so a tile makes its few only while showing them |
| `Views/KoanTable.swift` | The Mac's lists: an `NSTableView` of `TableRow`s, made of layers and labels. SwiftUI's `List` on macOS sets up every row in the data set before a page draws; a table makes the rows on screen. Selection, keys, drags and the SwiftUI menus hosted. iOS keeps `List` |
| `Views/TrackTableRow.swift` | The Mac's track row, for album pages, history and favourites: number or time, bars, sleeve, links, availability, heart, format, length — and day headings |
| `Views/QueueTableRow.swift` | The Mac's queue and playlist row: a record heading, or a track with its status, place or sleeve, availability, heart, codec and length |
| `Views/AvailabilityMark.swift` | Where a track's file is or how its download is going, as one layer every Mac row draws |
| `Views/MixedCollection.swift` | Favourites and search on the Mac: pills, record tiles and track rows in one collection view, each made as it scrolls in. Search's pick works across all three |
| `Views/LayerSymbol.swift` | SF Symbols drawn once into bitmaps for layers. One colour is a template, as SwiftUI draws it |
| `Support/RowMetrics.swift` | How tall rows are, on every list and both platforms |
| `Views/PickerSheet.swift` | ⇧⌘K picker: multi-select, add / add-and-play / replace queue |
| `Views/TransportBar.swift` | Transport, seek, format badge, output device |
| `Views/LyricsPanel.swift` | Synced lyrics highlighted against position |
| `Views/SettingsView.swift` | Library / Server / Playback / Devices / Appearance — everything needed to set koan up without a terminal |
| `Views/ActivityIndicator.swift` | The running-task rows at the foot of the sidebar |
| `Views/FavouriteButton.swift` | The heart, wherever something can be favourited |
| `Views/DevicePicker.swift` | Play on: pick a device to control, Move here to send the music there. While another device is controlled the engine publishes its state as the app's own — see ARCHITECTURE |
| `KoanIOS/RemoteActivity.swift` | The Live Activity for a device the phone controls, and the intent its buttons run. Compiled into the widget extension too (`apps/ios/Widgets`) |
| `Views/HistoryView.swift` | Play history, grouped by day — read-only, select and ⌫ to forget |
| `Views/PlaylistView.swift` | A playlist, laid out like the queue — grouped or flat, drag reorder, drop to add |
| `Support/PlaylistsModel.swift` | The playlists and everything done to them. Rows held whole; contents one at a time |
| `Views/OrganizeSheet.swift` | Organize: pattern + destination pickers, preview table with conflicts flagged per row |
| `Support/DspModel.swift` | EQ and convolution profiles, and the one import flow Settings, "Open in" and the iOS share extension (`apps/ios/Share`, via `KoanIOS/ShareInbox.swift`) all go through. `Views/DspProfilePage.swift` shows what a profile holds |
| `Support/OrganizeModel.swift` | Organize sheet state — debounced preview, generation-guarded so a slow plan can't land on a newer one |

### koan-server (`crates/koan-server/src/`)

| Module | What |
|--------|------|
| `graphql/mod.rs` | GraphQL schema builder, `KoanSchema` type, a bound on how many of the server's pooled connections resolvers hold at once, `with_db`/`blocking` offload helpers |
| `graphql/loaders.rs` | Dataloaders for artist→albums, album→tracks, counts, favourites |
| `graphql/jobs.rs` | Job registry for `triggerScan`/`triggerRemoteSync` — detached threads, polled via `job(id:)` |
| `graphql/queries.rs` | GraphQL query resolvers (artists, albums, tracks, nowPlaying, etc.) |
| `graphql/mutations.rs` | GraphQL mutations (playback, queue, favourites, playlists, organize) |
| `graphql/types.rs` | GraphQL type definitions (GqlArtist, GqlTrack, GqlNowPlaying, etc.) |
| `graphql/server.rs` | HTTP server (axum), `cmd_serve`, `start_api_background`, daemon mode, timeout/load-shed/panic-catch layers |
| `subsonic.rs` | Subsonic-compatible REST API (XML/JSON, auth, streaming, cover art), plus koan's `/rest/koanLink` WebSocket |
| `clients.rs` | Linked koan apps by account, and sending them `LinkCommand`s — what `clients`, `playOnClient` and `controlClient` use. Sends each link the account's other devices as they change, relays commands between them (`koanCommand` too), pushes Live Activity updates |
| `mcp.rs` | MCP server (schema_sdl + graphql tools): stdio for `koan mcp`, `/mcp` on the main port behind koan's own tokens, admin capped at `user` |
| `push.rs` | Apple push notifications to the iOS app: ES256 token auth, HTTP/2 to APNs. A background push wakes a suspended app to link; a play request becomes a notification to tap |
| `pair.rs` | Pairings waiting to be approved, in memory: id, Crockford code, device name, ten-minute lapse, the socket route that holds the device, and `settle`, which mints the approver's key and sends it down. `/rest/koanPairInfo` and `/rest/koanPairApprove` in `subsonic.rs` and the web UI's `/pair` (`ui/pair.rs`) settle through it |
| `transcode.rs` | Subsonic `stream` transcoding: whether a request gets the original or an `ffmpeg` Opus, MP3 or AAC encode, and running it |
| `share.rs` | Public share pages and their audio, answering for a share's own tracks only |
| `../styles/` | Tailwind sources for `assets/ui.css` and `assets/share.css`, on the theme koan.rocks uses (`site/src/theme.css`). The pages are styled with utilities in the templates; these hold element defaults and the rules for classes the scripts toggle or build (`playing`, `busy`, `missing`, the queue's rows). Quote every `class` attribute: Tailwind does not read an unquoted one. Run `just css` after changing either; the compiled files are committed because the crate embeds them |
| `ui/` | Web UI: server-rendered pages + Datastar, cookie-session gate, sign-in/resume/renew/sign-out, stream and cover routes. `assets/player.js` is the browser player both it and the share page use. `ui/oauth.rs` is the OAuth 2.1 authorization server for `/mcp`: discovery, stateless registration, consent, PKCE token exchange; `ui/connect.rs` the page explaining how to connect an assistant; `ui/scrobbling.rs` the page connecting an account to ListenBrainz |

### koan-cli (`crates/koan-cli/src/`)

| Module | What |
|--------|------|
| `main.rs` | CLI entry point (clap), logger (file + buffer), signal handling |
| `commands/play.rs` | `cmd_play` — orchestrates player spawn, queue restore, calls `run_tui()` |
| `commands/scan.rs` | `cmd_scan` |
| `commands/search.rs` | `cmd_search` (FTS5 with tree output) |
| `commands/remote.rs` | Remote login/sync/status |
| `commands/mod.rs` | Shared CLI helpers: `open_db`, formatters, path parsing, playlist builders |

## How to read the code

1. **Start:** `koan-core/src/player/state.rs` — the data model
2. **Then:** `koan-core/src/player/mod.rs` — the command loop
3. **Audio:** `audio/buffer.rs` (decode pipeline) → `audio/engine.rs` (CoreAudio setup)
4. **TUI:** `koan-tui/src/app.rs` (state machine) → `ui.rs` (render)
5. **Database:** `db/schema.rs` (tables) → `db/queries/sources.rs` (track identity)

## Concurrency patterns to follow

- **TUI→Player communication:** always via `PlayerCommand` through the crossbeam channel. Never reach into player internals from the TUI thread.
- **Player→TUI communication:** via `SharedPlayerState` (atomics + RwLock). The player thread sleeps until a command or a known event — the playhead reaching a queued track, a fade reaching silence — and `position_ms()` reads the playhead live. The TUI redraws on its own tick.
- **Audio thread (CoreAudio/cpal):** atomics and rtrb only. No allocations, no locks, no channels.
- **Decode thread:** owns the Symphonia decoder. Communicates via rtrb producer + `PlaybackTimeline` (RwLock for boundaries, atomics for counters).
- **Background work** (downloads, lyrics fetch, organize): spawn named threads, communicate results via crossbeam one-shot channels or `Arc<Mutex<Option<T>>>` polling.
- **Parallel iteration** (scan, remote sync): rayon. Don't hand-roll thread pools.

## Dependencies (key choices)

| Dep | Why chosen |
|-----|-----------|
| `symphonia` | Rust-native decoder, all codecs, gapless support |
| `rtrb` | Lock-free SPSC ring buffer for audio — the only bridge between decode and audio output |
| `coreaudio-sys` | Raw CoreAudio AUHAL bindings for bit-perfect output (macOS) |
| `cpal` | Cross-platform audio I/O — ALSA/PipeWire/PulseAudio (Linux) |
| `crossbeam-channel` | Bounded MPSC with timeout recv — command channel + one-shots |
| `parking_lot` | Faster RwLock/Mutex, no poisoning |
| `rusqlite` (bundled) | SQLite with FTS5 for full-text search |
| `lofty` | Tag read/write across ID3, Vorbis, MP4, APE |
| `ratatui` + `crossterm` | TUI framework + terminal backend |
| `nucleo` | Fuzzy matching (same engine as Helix editor) |
| `souvlaki` | Media key / MPRIS / Now Playing |
| `reqwest` (blocking, rustls) | HTTP client for Subsonic API |
| `rayon` | Data parallelism for scan + sync |
| `ebur128` | EBU R128 loudness measurement for ReplayGain |
| `realfft` | FFT for spectrum analyzer |
| `biquad` / `fft-convolver` / `rubato` | Parametric EQ, FIR convolution, resampling to an impulse response's rate |
| `async-graphql` | GraphQL schema derivation, execution engine |
| `axum` | HTTP server for GraphQL/Subsonic API |

## Roadmap

Active plans live in `.claude/plans/`. Key upcoming work:

1. **Tag editing** (plan 04) — vimv-style (TSV + $EDITOR) first, TUI inline editor second.
2. **DSP** (plan 02) — EQ and convolution are in (`audio/dsp/`); AutoEQ search/download, settings in the apps and crossfeed remain.
3. **Artist metadata** (plan 09) — bios, images, similar artists from MusicBrainz/Last.fm.

See `.claude/plans/README.md` for dependency graph and status.

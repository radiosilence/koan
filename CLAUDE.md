# Project Rules

## What is koan

Bit-perfect music player (macOS + Linux). Rust core, Ratatui TUI, plus native apps: SwiftUI on macOS, iOS and tvOS, Compose on Android, and the kōan GUI app (GPUI), built for macOS and Linux so far. Eight crates:

- **koan-core** — library crate. Audio engine, player, database, indexer, format strings, file organization, remote (Subsonic/Navidrome) client, shared helpers. No UI code, no terminal deps.
- **koan-tui** — library crate. Ratatui TUI, visualizers, media keys. Exports `run_tui()`. Depends on koan-core.
- **koan-server** — library crate. GraphQL (async-graphql + axum), Subsonic REST API, MCP server. Depends on koan-core.
- **koan-runtime** — library crate. `KoanEngine`, the facade every app drives: the offload lanes, the state stream and its slices, queue patches, settings, DSP, devices, sharing, and the records it speaks in. Plain Rust, no uniffi. Depends on koan-core, and on koan-server to share the device's library (`serve.rs`). Not published.
- **koan-ffi** — library crate, built as a staticlib (Apple) or cdylib (Android) by the native apps' builds (`cargo rustc --crate-type`). The uniffi bindings over koan-runtime for Swift and Kotlin, and nothing else: each record, enum and error re-declared for `uniffi::remote` (checked field for field against koan-runtime's), the objects wrapping koan-runtime's and delegating, bridges for the traits the apps implement, and Android's JNI glue. It stays a crate of its own because uniffi names every symbol after the crate that exports it. Not published.
- **koan-gui** — the kōan GUI app (`koan-gui`), drawn with GPUI through koan-kit. Desktop builds for macOS and Linux; Windows waits on koan-core (#1600). Depends on koan-runtime as plain Rust and drives `KoanEngine` as the Mac app does. Draws only the kōan look, from koan-kit. The library pages are `src/library/`. Not published.
- **koan-kit** — the design system the GUI app is built from: the theme (`src/theme/`: tokens, Geist Mono, the icon set, the record accent, the wash) and the components (`src/components/`). Knows nothing of the engine; `cargo run -p koan-kit` opens its storybook of every component. koan-gui reaches GPUI only through it: pages are written in `koan_kit::app` (the app model and `div`), and anything else is a named `koan_kit::raw` import with a comment saying why, which `just gui-theme-leaks` counts and enforces. Design: `docs/design/koan-kit.md`. Not published.
- **koan-cli** — binary crate (`koan`). Thin entry point: clap CLI, logger, signal handling, command routing. Depends on koan-core + koan-tui + koan-server.

Plus **tools/uniffi-bindgen** — the Swift bindings generator (`just ffi-bindings`). A crate of its own so that running it builds uniffi and nothing else; it reads the bindings from any built koan-ffi library, the iOS one included.

Plus **apps/macos** — SwiftUI app (SwiftPM, Swift 6, macOS 26+). Links koan-ffi.

Plus **apps/android** — Compose app (Gradle, Kotlin, Android 10+). Loads koan-ffi as a shared library through uniffi's Kotlin bindings; Gradle builds it with cargo-ndk.

Dependency rules (compiler-enforced): front ends hold no business logic, which lives in koan-core. koan-tui and koan-server depend only on koan-core; koan-runtime embeds koan-server as koan-cli does, starting and stopping it on the app's own player; koan-ffi depends only on koan-runtime, and nothing but the native apps' bindings depends on koan-ffi; koan-gui depends on koan-runtime and koan-kit, and nothing else depends on it; koan-kit depends on neither the engine nor kōan's data. Native clients reach koan-core through koan-ffi, the GUI through koan-runtime. Something a GUI page needs that `KoanEngine` lacks goes into koan-core and koan-runtime, with its binding in koan-ffi, so the Mac app could share it.

**Local UI goes through FFI, not GraphQL.** The macOS app links the engine in-process — a daemon, a port and an auth surface buy nothing when the UI is sitting on top of the audio engine. GraphQL is the surface for clients that genuinely can't link the core: the TUI's remote bridge, the MCP server, scripts and jukebox remotes. The web UI is server-rendered and calls koan-core in-process. The iOS and Android apps link the core like the Mac app does. When adding a capability to one, consider whether the other needs it too — both are thin shims over the same koan-core helpers.

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
- **Queue items carry the track's uid** — a row id is reused once its row is deleted, so after the library loses rows the queue finds its tracks again by uid (`helpers::queue_relinks`, `SharedPlayerState::relink`). An item whose track is gone is unlinked and plays from its own path and metadata.
- **Status is derived** — `QueueEntryStatus` computed from the cursor, the row's `played` mark and its load state. Played is recorded when a track plays, never inferred from position.
- **Shuffle never moves the queue** — it is a play order beside it (`PlayOrder` in `player/state.rs`) that the advance and the decoder's lookahead both read.
- **Decode cursor ≠ UI cursor** — decode thread peeks ahead for gapless without moving the playlist cursor.
- **One `derive_visible_queue()` per frame** — cached snapshot, all render/mouse ops see consistent state.
- **Tracks are derived from sources** — each file is a `local_files` row and each server entry a `remote_entries` row, holding its own tags. A track holds at most one of each; its columns are derived, the file's first. `sources::link` alone decides which track a source is (MusicBrainz recording + release, or album/album artist/disc/number/title with the artist as tie-break; ambiguous → declined). Never write track identity columns outside `db/queries/sources.rs`. Albums are title + album artist + release (editions are separate albums); artists are their folded name; favourites reference rows by id.
- **Credits and record runs are core's** — every artist a client draws is a credit built in `helpers/credits.rs` (each credited artist linked, "Unknown Artist" for a record naming none), whether a row names its artists is `RowCredits::differ`, and which rows sit under one record heading is `helpers::record_runs`. The FFI carries the results (`credit_differs`, `run`); clients draw them and compare no names.
- **Uids, not row ids, leave the database** — every artist, album, track and playlist has a UUIDv7 `uid`, published by Subsonic, GraphQL and the link. Clients syncing from a koan server adopt its uids, so ids mean the same thing on every device. See `db/queries/uids.rs`.
- **Figment-layered config** — defaults → `config.toml` → `config.local.toml` → `KOAN_*` env vars. All writes go through `Config::persist()`, which diffs the mutation and routes each changed key by `config::layer_of` — secrets, this machine's paths/hardware/account and volatile UI state to `config.local.toml`, taste to `config.toml`. Comments survive; untouched keys are never rewritten.

## Git

- Work in PRs; never push to main. PRs are squashed; merge main into a branch rather than rebasing.
- Never push tags or run `cargo publish`. CI tags and publishes when a release PR merges.
- Commits are signed.
- `Checks` is the only status the ruleset requires. CI jobs run per domain the change touches and may skip; add a new CI job to `checks`' needs, never to the ruleset.

## Changelog

`CHANGELOG.md` is generated by `scripts/changelog.py` and never edited by hand. Every change adds one fragment under `changelog.d/unreleased/<section>/`, named `<pr>-<slug>.md` and holding one `- **Lead.** Detail.` entry; to correct a note, edit its fragment. `just changelog --stdout` previews the result. The format is in `changelog.d/README.md`.

## Releasing

`just release patch|minor|major` (or an exact `X.Y.Z`) branches from main, bumps the workspace version and the internal crates' pins in `Cargo.toml`, `Cargo.lock`, `deploy/pulumi/package.json`, `deploy/pulumi/src/versions.ts` and the Android `versionName`, turns the unreleased fragments into the version and opens the release PR; `gh workflow run release.yml` does the same from CI. Merging it is the release: CD, the half of `ci-cd.yml` after the `Checks` gate, publishes the crates, the container image, the Pulumi package, the GitHub release (binaries, the macOS app, the Android APK, the kōan GUI app's DMG, AppImages and .debs, and the changelog section as notes) and the Homebrew formulae. Its iOS and tvOS TestFlight builds join the public TestFlight group and go to beta review; builds from other pushes stay internal. A push releases only when it changes the version; `gh workflow run ci-cd.yml` on main retries a release that failed part way. The App Store is separate too: once a release's TestFlight builds have processed, `gh workflow run store.yml` pushes `apps/ios/store/listing.toml` and submits iOS and tvOS to App Review. The server deploy is triggered separately, with `gh workflow run update-apps.yml -R radiosilence/jaritanet` once `gh release view vX.Y.Z` succeeds.

## Docs and site

`docs/` is the user guide and reference, and koan.rocks is built from it (`site/src/docs.sh`, deployed by `site.yml`). A change in behaviour, a setting, a CLI flag or a supported platform updates the matching page in the same PR, as does a changelog fragment. Before a release, check that the guide still describes what the code does, since screenshots and settings drift fastest.

`docs/reference/protocol/` specifies the protocol: the link, nearby connections, pairing and invites, every `koan*` extension, and the compatibility rules. A change to it (`EXTENSIONS` in `subsonic.rs`, the constants in `remote/profile.rs`, a `/rest/koan*` route, a field or variant of `LinkCommand`, `LinkReport`, `PairMessage` or the nearby frames, or the TXT entries) updates the matching page in the same PR, and keeps the compatibility rules on `protocol.md`. The guide pages that describe the same endpoints for people (`authentication.md`, `devices.md`, `dsp.md`, `headless-server.md`) are updated with it, and `just protocol-schema` regenerates the JSON Schema in `docs/reference/protocol/schema/`, which the tests check against the types.

## Supervising sessions

If you are told you are the foreman, supervisor, manager, boss or guvnor of the koan sessions, in any wording, this section is your brief: read it before doing anything else. Other sessions skip it.

The manager coordinates and does not write features. It keeps the context, makes the decisions and hands each piece of work to another session as a self-contained task: the exact branch, the exact change, the command that verifies it, and what to report back.

- **Orient first.** Run `date`, then `gh pr list`, `gh issue list`, `git worktree list`, `gh release list --limit 3` and `ListAgents`. The open PRs and their CI state are the work in flight. Main's last `release: vX.Y.Z` commit tells you what has shipped since.
- **Workers.** Sessions named `worker-<name>` each run in `../worktrees/koan/worker-<name>` and switch to whichever PR branch they are given. Other named sessions own their own area, such as the Apple TV app, and get status requests and briefs rather than unrelated tasks. A session that has hit its context limit is cleared by the user and then re-briefed by the manager with everything it needs in one message. A compacted session loses detail, so prefer clearing.
- **Merging.** The manager merges green PRs when the user asks it to (`gh pr merge --squash`). When PRs conflict, merge main into the branch and push. The manager's token cannot merge a PR that changes `.github/workflows/`, so the user merges those.
- **Releasing.** Cut a release once the in-flight PRs the user wants are on main, as described under Releasing above. When the release is published, trigger the server deploy and confirm the TestFlight builds uploaded.
- **Docs.** Before cutting a release, have a session check `docs/` and the site against what merged since the last one.
- **Housekeeping.** Remove a PR's worktree and its `target/` once it merges or is abandoned; Cargo target directories run to gigabytes each. Keep at most one booted simulator across all sessions, never drive or capture the user's live screen, and stop only the processes you started.
- **Secrets.** Signing and App Store Connect credentials come from the 1Password service account (`op`). Export a key to the scratchpad only for the command that needs it, and delete it afterwards. Never write a credential into the repo, a PR or an issue.

## Build & check

```bash
just check          # cargo test + clippy -D warnings + actionlint
just fmt            # cargo fmt
just release VER    # X.Y.Z or patch/minor/major: branch, bump, changelog, commit, PR
just theme-leaks    # styling in the apps that bypasses the kōan theme's roles
just protocol-schema # regenerate docs/reference/protocol/schema/ from the message types
just cli            # cargo run --release -p koan-cli -- <args>
just build          # cargo build --release
just macos-run      # build + launch the macOS app
just macos-dmg      # package the app for release
just css            # compile the web UI's Tailwind stylesheets (output is committed)
just js             # typecheck the web UI's TypeScript (cargo strips the types itself, in koan-server's build.rs)
just ios-typecheck  # the shared SwiftUI sources still build for iOS
just tv-typecheck   # the same for tvOS
just macos-test     # the macOS package's Swift tests
just ios-run        # build and launch on a booted simulator
just ios-smoke FILE # play a file through the real Player on the simulator
just ios-phone      # install on the plugged-in iPhone (personal team)
just ios-walk       # UI test that screenshots every page, into target/ios-walk
just ios-bars       # every scrolling page clears the kōan bar, on an iPhone and an iPad, against a generated library
just ios-testflight BUILD # archive, sign and upload to TestFlight (needs the ASC key)
just ios-signin DEV URL USER PASS # sign a simulator in through Settings, as App Review does
just ios-invite-sheet # an admin's invite sheet stays up and its QR code reads back as the link
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
just android-ffi [ABIS] # koan-core for Android via cargo-ndk, and the Kotlin bindings
just android-run [ABIS] # build, install and open the Android app on the device adb is connected to
just android-apk [ABIS] # the sideloadable release APK CI builds, signed with the debug key
just android-walk [AVD] [SEARCH] # every page, both looks and appearances, on an emulator against a generated library, into target/android-walk
just gui-run    # the kōan GUI app from source, against target/gui-config unless KOAN_CONFIG_DIR is set; Metal shaders compiled at startup (runtime-shaders), so no Xcode; on macOS launched as a dev bundle, so it has Local Network permission of its own
just vendor-gpui VER [KIT] # the vendored GPUI crates rebuilt from release VER (gpui-base from gpui-kit's KIT) + vendor/patches; fails loudly on a hunk that does not apply
just vendor-gpui-patches # vendor/patches written from the vendored copies, after editing one
just gui-smoke  # the GUI app headlessly as used: playing, seeking, hovering every page; fails on a panic, a hang or no track change
just gui-walk   # every GUI page headlessly: PNGs dark and light, scroll frame times, fails over 6.9 ms at p95
just gui-compare # the SwiftUI app's offscreen renders beside the walk's, edges that disagree marked
just gui-parity # the parity inventory: the Mac app's views, actions, settings, tooltips as a checklist
just gui-build  # release build of the GUI app
just gui-library DIR [ALBUMS] [TRACKS] # generate and scan a large throwaway library for the GUI benches
just gui-theme-leaks # koan-gui's reach past koan-kit, against the ratchet in crates/koan-gui/theme-leaks.toml
just gui-bundle # package the desktop app for this machine: .app + DMG on macOS, AppImage + .deb on Linux
```

The macOS app needs `just macos-ffi` to have run at least once — it generates the Swift bindings that `swift build` compiles against. `macos-build` does this for you.

Development builds are signed with a self-signed certificate — `just macos-signing-cert` creates it, once. Without it the app is ad-hoc signed, which derives its identity from the binary's own hash: every rebuild is a different application to macOS, so TCC permissions are forgotten each time. It buys nothing against Gatekeeper, which wants Developer ID and notarisation.

Pre-push hook (`.claude/settings.json`) runs `cargo fmt --all` before any `git push` and refuses the push if it changed anything. Clippy and the tests are CI's, so a push is never held up running them locally.

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
| `audio/android_backend.rs` | Android `AndroidAudioBackend`: AAudio declared by hand, the stream opened in the encoding `bit_perfect` chose |
| `audio/bit_perfect.rs` | Bit-perfect USB output on Android 14+: which of the DAC's mixer formats to hold for a stream (`choose`), the exact float-to-integer conversion, and the app's `OutputHost` |
| `audio/engine.rs` | CoreAudio output setup, render callback. AUHAL on macOS, RemoteIO on iOS — two properties apart |
| `audio/buffer.rs` | `PlaybackTimeline`, track boundaries, decode thread entry points (`start_decode`, `decode_queue_loop`, `decode_single`) |
| `audio/device.rs` | CoreAudio device enumeration, sample rate get/set (macOS only) |
| `audio/replaygain.rs` | EBU R128 loudness scanning, gain application via lofty |
| `audio/dsp/` | EQ and convolution per output device, on the decode thread. No profile, no chain: bit-perfect stays checkable. Delays trimmed and flushed so the timeline counts output time. `steps.rs` runs a profile's filters in order — bands, delays, mixes, graphic curves as minimum-phase FIRs — since a mix does not commute; `impulse.rs` is the routing matrix (Convolver's model; a WAV is its diagonal); `import.rs` sorts files, folders and zips into one profile through `convolver`, `camilla`, `apo` and `raw`; `profiles.rs` is what every front end calls; `edit.rs` the rules of an edit by hand (the ranges, the band types, the brush) every editor clamps and paints by |
| `audio/viz.rs` | `VizBuffer` (ring of f32 samples for analyzer), `VizSnapshot` (atomic snapshot for UI), `VizLevels` (the spectrum as three bands, for callers that poll often and draw little), `BarHeights` (a playing indicator's bars from them, each band against its own recent loudest; koan-ffi exports it for every app) |
| `audio/analyzer.rs` | FFT analysis thread — 48-band spectrum, VU meters, peak hold. Runs at configurable FPS. Its spectrum, VU and loudness show what the listener hears: fed after ReplayGain and DSP, and scaled by the gain the output last applied (`fade::heard_gain`: the pause fade, a sleep timer, mute) |
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
| `index/cover_files.rs` | Cover images beside the tracks as the last full scan saw them (`cover_files`); one added, replaced or removed names the records under it in `art_evictions`, since no track row changes |
| `index/folder_art.rs` | Covers as image files beside the tracks (`cover.*`, `folder.*`, `front.*`, Navidrome's order), ahead of embedded art. `cover_art` is what every cover lookup calls. Directory listings kept against the directory's mtime |
| `index/id3v2_pictures.rs` | MP3 tag reads with the embedded art held back — walks the ID3v2 frame headers and serves lofty zeros over the picture frames it would only discard |
| `format` | fb2k-compatible template engine, re-exported from sift (`sift-music`) — the tagger shared with other importers. Change it there |
| `remote/client.rs` | Subsonic/Navidrome HTTP client (reqwest blocking, MD5+salt auth) |
| `remote/download.rs` | Streaming downloads: `.part` → verify → atomic rename, progress, retries. All disk-bound remote bytes go through here |
| `remote/sync.rs` | Library sync: album list, then songs paged in bulk via empty-query `search3` (per-album `getAlbum` for servers that cannot), one transaction per page, progress per page. Every sync walks everything; `helpers::sync_remote` decides whether to walk, by the server's `getIndexes` `lastModified` |
| `remote/history.rs` | Play history shared through a koan server: the outbox of scrobbles and forgettings (offline plays reach the server dated to when they started), and adopting the account's other devices' plays after a cursor. Gated on `koanHistory` |
| `remote/link.rs` | The standing WebSocket a client keeps to a koan server (`/rest/koanLink`), and the `LinkCommand`s the server sends down it: play, enqueue, pause, skip. Reconnects on its own; ids are resolved to local tracks, syncing first if one is new |
| `remote/relay.rs` | The app's side of kōan's push relay: per server a slot, the handle and the revoker, registered through `koan-relay-client` off the link's thread, with App Attest from the app (`Attester`); Live Activity handles. Servers get handles, never tokens |
| `remote/pair.rs` | A device without a keyboard signing in: opens `/rest/koanPair`, shows the code and `koan.rocks/pair/` link, and blocks until the server sends the outcome; an approved key is stored as an invite's is. Also the approver's calls (`info`, `approve`, `decline`) and `PairLink` |
| `remote/profile.rs` | What the signed-in server is: `ping` + `getOpenSubsonicExtensions`, once per sign-in. Gate koan features on the extension (`koanLink`, `koanDevices`), never on the server's name |
| `remote/scrobbling.rs` | The account's scrobbling on its koan server (`koanScrobbling`): status, connect with a ListenBrainz token, disconnect. The server keeps the token and never returns it |
| `remote/connections.rs` | Who is connected to this device and whom it is connected to — the server link, the account's devices behind it, nearby sessions either way — for Settings to list and end |
| `remote/devices.rs` | The devices this one can play on — the account's from the link, the network's from `nearby` — which one the app controls, and getting a command to it |
| `remote/nearby.rs` | LAN control: listener on `devices.port`, Bonjour via `dns_sd` (Apple) or the app's `NsdManager` via `nsd` (Android), a connection per device found or listed by address. Strangers get playback and the queue only (`LinkCommand::allowed_nearby`) |
| `remote/wire.rs` | Event-driven WebSocket sessions: one `poll` on the socket and a pipe the engine's change signal rings |
| `remote/wikimedia.rs` | Wikidata items, Wikipedia lead sections and Commons images — where artist bios and photos come from |
| `remote/covers.rs` | The server's covers kept on the device: the `artwork` directory in the download cache, one file per cover id and size bucket, read through by every cover lookup (`cover`, `kept`/`keep`), counted against the cache limit and trimmed after the downloads. The preload: a thread asleep on the change signal that fetches every server-only record's 600 px cover while no page's cover is in flight, two hundred at a time through `koanCovers` where the server has it (stopping between frames when it must, and pausing while a page's covers load; a cut stream asked again for what it did not reach) and six `getCoverArt`s at a time otherwise. Its requests take background slots, which wait for no page's cover in flight and never count against a page's, with progress for the `Covers` slice |
| `remote/queue.rs` | The download queue: what to fetch when. Follows the playlist (started by `Player::spawn`; front ends never enqueue), worker pool, a priority lane for the track under the cursor, cursor-aware reordering. With a cache limit, fetches only as far ahead of the cursor as the limit allows (the playback window), and trims the cache |
| `remote/downloads.rs` | The download store, owned by `SharedPlayerState`: the one table of transfers, keyed by track, with every queue entry waiting on each. `settle` is the one way a transfer's end is told |
| `upnp/` | Playing to UPnP/DLNA MediaRenderers. SSDP discovery, SOAP, GENA, DIDL-Lite, the tokenised HTTP listener renderers fetch from, and `stream.rs`, which encodes a processed session for one. Hand-rolled on std threads; an event is a reason to ask the renderer where it is, never a reading |
| `quiet.rs` | What runs in the background on iOS: nothing nobody asked for. Link, nearby browse and dial, sync and rescans wait here; a phone playing stays findable. Lifted by controlling another device or a push |
| `places.rs` | Where a front end keeps runtime files outside the config directory: the platform's state, runtime and cache directories under `koan/<key>`, the key naming the config directory, and `adopt` for moving an earlier version's files there |
| `config.rs` | Figment-based layered config: defaults → config.toml → config.local.toml → KOAN_* env vars |
| `helpers/` | Shared by every front end, one module per concern and re-exported as `helpers::*`: `sign_in`, `watch` (folder watching, auto-sync), `sync`, `library` (rebuild, forget-folder/forget-remote), `cache`, `download`, `favourites`, `shares`, `playlist_items`, `credits` (credits and whether a row names its artists), `runs` (which rows are one record), `text` |
| `playlists.rs` | Playlists beyond the database: two-way Subsonic reconciliation, background pushes, M3U8 export. Read-only playlists (smart ones here, `readonly` ones there) are never pushed |
| `smart.rs` | Smart playlist rules: the typed model and its JSON, checked with errors that name the problem, and Navidrome `.nsp` parsing into it |
| `shelves.rs` | Favourites, Recently played and a search as filters on the album, artist and track listings (`AlbumQuery`, `ArtistQuery`, `TrackFilter`). A shelf page's previews are the head of those listings, with counts from the same query, so a preview and its "See all" agree. What each shelf holds, its window and its order live here; front ends ask by name |
| `organize.rs` | File rename using format strings. Preview/execute/undo — one `PlanEntry` per file carrying its destination and outcome. Moves ancillary files |
| `lyrics.rs` | LRCLIB lyrics fetching and parsing (synced LRC + plain) |
| `scrobbling.rs` | Forwarding plays to ListenBrainz and Last.fm (`scrobbling/lastfm.rs`: signed calls, the sign-in round trip, the API key from `[lastfm]`): one sleeping thread, woken when a play is queued, sending the durable `scrobble_outbox` in batches and backing off only while the service is unreachable. Now-playing notices are best effort |
| `artist_info.rs` | Artist bio and photo: MusicBrainz id → Wikidata → Wikipedia/Commons. Resolved by id, never by name alone; cached per artist, misses included |

### koan-tui (`crates/koan-tui/src/`)

| Module | What |
|--------|------|
| `play.rs` | `run_tui()` — TUI event loop entry point, frame timing, input handling |
| `app.rs` | `App` state machine, `Mode` enum, event handlers per mode |
| `ui.rs` | Render pipeline: layout → transport → content → overlays → hints |
| `rainbow.rs` | The Mac's hidden appearance mode (`Support/Rainbow.swift`): the code that switches it, kept per device, the accent's palette cycle (a timer only while drawn, in front and with motion), the sheen the roots put in the wash, told them through the theme (`KoanTheme::prism`, kept there alone), and the tracks that bring it out; drawn on the GPU through koan-kit's prism, which the roots mount (`root.rs`) and run only while playing, in front and shown, the sheens and gradients with it (`Prisms`), the analyser feeds (`levels::prism`) and `celebrate` throws confetti from |
| `transport.rs` | Transport bar widget: seek bar, track info, click-to-seek |
| `queue.rs` | Album-grouped queue with status icons, selection, drag targets |
| `nav.rs` | Where the TUI is: one page, the history of pages visited, a cursor — browser-like, as the apps' `Navigator` |
| `shell.rs` | What every page sits in: gutters, the toolbar row, text measured and cut to width, links to artists and albums |
| `browse.rs` | The library's browsers (Albums, Artists, Tracks) and an artist's page, narrowed by the browse filters and ordered by the sort |
| `picker.rs` | Nucleo fuzzy search, multi-select, colored matches |
| `cover_art.rs` | Halfblock rendering (2px per terminal cell, Lanczos3 resize) |
| `visualizer.rs` | Spectrum analyzer widget (reads `VizSnapshot`) |
| `lyrics.rs` | Lyrics side panel — synced line highlighting, scroll |
| `organize.rs` | Organize modal: pattern picker → preview table → background execute |
| `media_keys.rs` | macOS Control Center via souvlaki, manual CFRunLoop pump |
| `enqueue.rs` | `enqueue_playlist()` — build PlaylistItems from track IDs, submit downloads |
| `remote_bridge.rs` | Remote control: mirrors a koan server's now-playing and queue into the TUI over GraphQL and sends its commands there. No local playback |

### koan-runtime (`crates/koan-runtime/src/`)

| Module | What |
|--------|------|
| `lib.rs` | `KoanEngine` — the whole facade. Transport, queue ops, library queries, favourites, playlists, devices, scan. Every call that can block is `async`; only single-atomic reads stay sync, apart from `appearance()` and `flush_settings()`, which can wait for queued settings writes (see ARCHITECTURE) |
| `offload.rs` | Where blocking work goes — a growing thread pool for reads, one ordered lane for anything that ends in a `PlayerCommand` and for playlist edits, one for commands from other devices and one for settings writes, which `Batches` queues and coalesces |
| `state.rs` | The engine's state as slices, and one cursor per client. Whole snapshots, batched at a tick, cut by rate of change |
| `queue_slice.rs` | The queue as clients are sent it: whole on an edit, and between edits a patch of the rows whose status moved |
| `serve.rs` | Sharing this device's library: koan-server's `ApiServer` on the engine's player, the first account, and the loopback client the account settings administer it through while it runs |
| `types.rs` | Records mirroring koan-core types (`Track`, `Album`, `NowPlaying`, `QueueItem`, …): owned, concrete fields, as the bindings need them, and the conversions |

### koan-ffi (`crates/koan-ffi/src/`)

`lib.rs` is the whole of it: koan-runtime's records, enums and errors re-declared under `#[uniffi::remote(...)]`, the objects (`KoanEngine`, `StateStream`, `VizStream`, `BarHeights`, `OrganizeSelection`) wrapping koan-runtime's with exports that delegate, the traits the apps implement (`ProgressReporter`, `PushAttester`, the audio session and output hosts, `NearbyHost`, `MulticastHost`) with bridges into koan-runtime's, and the free functions. A change to the facade's surface is made in koan-runtime and repeated here: the compiler checks a remote record against its original, and the delegating method is one line. `android.rs` is the JNI glue. `uniffi.toml` names the Kotlin package.

Swift bindings are generated, not checked in — `just macos-ffi` builds the lib and regenerates them.

### koan-gui (`crates/koan-gui/src/`)

GPUI through koan-kit only: pages are written in `koan_kit::app` and build from `koan_kit::components`; anything else from GPUI is a named `koan_kit::raw` import with a reason, counted against `theme-leaks.toml` by `just gui-theme-leaks`, which also forbids `gpui::` in source (the `gpui` dependency exists only for the code GPUI's macros write). A window's root is the wash and the content as two cached views, so a drift frame redraws the wash alone; see ARCHITECTURE.md. Nothing blocks the main thread: engine futures are awaited from `cx.spawn`, CPU work runs on the background executor, and the main thread applies finished results and draws.

| Module | What |
|--------|------|
| `app.rs` | Installs the globals (`install`) once `main.rs` has opened the engine and started `platform::init`, and opens the window once the restored rows arrive or after half a second; the session is kept only once the restore succeeded. Quit saves the window's bounds; `platform` saves the session. Invite and pairing links from outside (`platform::Links`) go to Settings (`settings::answer`) |
| `root.rs` | A window's root: the wash and the content as two cached views; the playing record's sleeve, decoded off the main thread, sets the accent and the wash |
| `paint.rs` | `Paint::of(window, cx)`: the theme's `Look` with its colours flattened for the views |
| `mirror.rs` | The engine's state slices, one GPUI entity each, fed by one task on `StateStream::next()`. A queue sent whole is indexed and laid out as rows on the background executor; patches are applied relative to their base |
| `engine.rs` | The `KoanEngine` global and `command`, which runs a command off the main thread and logs failures |
| `navigator.rs` | Where the app is: `Route` (plain data, ids only), and the `Navigator` entity holding the history, the cursor and each route's rewinds; `choose` sends the page showing to its top. Every navigation rule lives here, with its tests |
| `shell.rs` | The window: the sidebar from the top edge, the window's buttons on its ground, beside the page column (toolbar with back and forward, then the stage); transport along the foot. "Wash the whole window" takes the sidebar's and toolbar's ground away and spans the transport under both columns |
| `sidebar.rs` | The queue, the library's sections, the playlists |
| `sidebar_status.rs` | The sidebar's foot: what the device plays from and the library's counts, from `library_stats` on the Library and Cache slices |
| `queue/` | The queue page, kept mounted: rows from `QueueItem.run` grouped or flat (`rows.rs`, disc numbering there too), linked credits, the heart, the availability mark with a transfer's ring, selection by queue item, keys, drag reorder, the rows that land with an edit lifting for a moment, context menu |
| `lyrics.rs` | The lyrics beside the page (koan-kit `Inspector`, `SyncedLines`, whose glide and emphasis the GPU draws, timed from when the playhead reached each line): cache first, LRCLIB after, words held with their track; one timer to the next line from the playhead anchor, none while paused; a click on a line seeks |
| `artwork.rs` | The artwork viewer over the window, opened by a record page's sleeve and the transport's cover; decoded at the size drawn by the shared sleeve cache; a click or Escape closes it |
| `links.rs` | Names that lead to their pages: a record's title, each credited artist. The queue and the transport draw with it |
| `transport.rs` | The transport bar, its names linked to their pages, and the seek slider, whose anchor is handed to the slider's `Glide` so the GPU moves the fill each present, and whose times follow the engine's whole seconds (`ClockStream`), with no timer of its own; the control, output, sleep and EQ menus. The output menu searches for renderers as it opens and refills as they answer |
| `devices.rs` | Play on: the control menu (this device, the account's and the network's), move here, and what became of a move; `can_move_music` in koan-runtime decides where music can go |
| `tv_wake.rs` | Pairing an Apple TV to be woken from the account's devices, as the Mac's `TvWake`: the TV shows a code, this device types it; "stop waking" in Play on undoes it |
| `server_banner.rs` | The line across the top of every page while the server cannot be used (`ServerStanding`), redrawn only when the standing changes; Sign In opens Settings on the server |
| `organize.rs` | Organise: the sheet over the window for a pick's files, a pattern previewed as typed (`OrganizeSelection::generate`) and checked against the disk once typing rests, every destination listed, a confirmation with the counts before `organize_execute` |
| `notice.rs` | The one line at the sidebar's foot the app says something in: a move's outcome, a command another device did not take (the Connection slice's `command_notice`) |
| `downloads.rs` | The downloads page: transfers from the Transfers slice, a running row's bar filled on the GPU from its transfer's `Glide` (the indeterminate stretch while the size is unknown), rates from Figures; retry, cancel, remove a file and clear finished, each asked first; the cache's size against its limit. Made on arriving and dropped on leaving |
| `transfers.rs` | Each running transfer as a koan-kit `Glide` (its share at the last report and its rate), fed from the mirror's figures, from which the availability rings and the seek bar's downloaded part fill on the GPU, and the downloads page's bars too; each row's `Availability` |
| `covers.rs` | Sleeves per record and size, decoded off the main thread to the renderer's pixel format |
| `levels.rs` | What moves the playing bars koan-kit draws (`PlayingBars`): one analyser subscription while any bar is on screen in a window that shows (the roots report hiding and showing), at 30 Hz, each frame a keyframe the bars' shaders glide to at the display's rate, so a moving bar presents without drawing a frame; where bars are quads, only the roots of windows holding them paint again |
| `session.rs` | Saving the session while playing, when playback stops, and on queue edits |
| `window_state.rs` | The window's bounds, whether the sidebar shows and whether the queue is grouped, across launches, in `desktop-window` in the configuration's state directory (`places::state`) |
| `time.rs` | A duration as a clock: `m:ss`, or `h:mm:ss` from an hour |
| `measure.rs` | With the `profiler` feature: `KOAN_GUI_MEASURE=scroll` scrolls the queue through the window and prints GPUI's frame histograms; `fling` flings the album grid to its end and back and prints the process's footprint at each stage; `play` (filling an empty queue from the library first), `hidden` and `play-hidden` set the app up for its CPU use to be read from outside; `frames` and `play-frames` count the frames asked for, skipped and drawn over 20 s |
| `examples/walk.rs`, `examples/compare.rs` | `just gui-walk`: every page offscreen against a throwaway library, a PNG of each named as the Mac's evidence renders are, and each page's frame times (layout and paint) while scrolled by wheel events; `just gui-compare` sets those beside the SwiftUI app's `KOAN_RENDER_EVIDENCE` renders with the edges either draws alone marked |
| `examples/smoke.rs` | `just gui-smoke`: the real engine on a muted, generated library, playing, seeking and pausing while the pointer sweeps every page in both appearances, rainbow on; fails on a panic (101), a hang (124) or no track change. Refuses any configuration but a throwaway one |
| `examples/window_shot.rs` | Draws the real window offscreen, dark and light, into a PNG, against a throwaway `KOAN_CONFIG_DIR` (`KOAN_SHOT_TRACKS=n` fills and saves a queue first) |
| `examples/` (others) | `settings_shot.rs` (each Settings pane to a PNG), `sheet_shot.rs` (each sheet over the window, dark and light: search, shortcuts, artwork, organise, a confirmation, a name; `KOAN_SHEETS` picks), `cold_covers.rs` (a screen of sleeves from a cold cache), `settings_frames.rs` (offscreen frame times), `filter_check.rs`, and `gen_library.rs`, which `just gui-library` runs |
| `koan-kit: app.rs` | The app model pages are written in: entities, contexts, views, events, actions, focus, tasks, `div` and its layout traits, the platform services the app uses, and `a11y` (roles, actions, live regions) for what a page tells a screen reader that no component says. What a page sizes, colours or draws by hand is not here |
| `koan-kit: lib.rs` | `raw`, GPUI re-exported whole for what `app` and the components do not cover. koan-gui takes from it only in `dsp/graph.rs` and `measure.rs`, each use with its reason |
| `koan-kit: theme/mod.rs` | `KoanTheme` (text size, "show icons"), `AccentFeed` (the record's accent, drawn through four colour slots the GPU mixes in OKLab as it settles; what reads it on the CPU calls `observe_accent` and hears once it lands), `Look::of(window, cx)` (a window's appearance and tokens), `KoanStyled::text_role`, `drive` for a window's root, Reduce Motion, Increase Contrast, Reduce Transparency and the text size read from the system (`set_system_display`; on Linux the desktop portal), which `Look` carries as `contrast`, `opaque` and `hairline`, with `veil` for a dim that stays opaque when asked, gpui-component's theme dressed in the tokens for its `Input` |
| `koan-kit: theme/tokens.rs` | Colours, type roles, spacing, hairline, motion durations and the quick-out curve |
| `koan-kit: theme/accent.rs` | `Color.dominant` and the OKLCH tone mapping, value for value with Swift and Kotlin; `Shade` carries the slots it is drawn through |
| `koan-kit: theme/icons.rs` | `KoanIcon` and the asset source serving each glyph at its stroke width; `icons/glyphs.rs` is generated by `just icons` |
| `koan-kit: theme/wash.rs` | The wash, drifting only while asked and allowed. Where the renderer draws shaders (`SHADED`) the GPU makes it: `field` reduces a record once to a 32-texel field and the `WASH` program reconstructs it and applies the tone's hold, the ground and the dither at each present, its drift, turn and dissolve keyframes the program reads at present time. Elsewhere `bake` (Android's `WashBake`, BGRA out) and one textured quad per frame |
| `koan-kit: components/animated.rs`, `bars.rs` | `Animated` and `AnimatedQuad`: motion the GPU draws from keyframes (tracks: from, to, start, length, curve) written on any thread when the data changes and interpolated by the program at present time (`koan_track`); `PlayingBars` and `BarLevels` on it, the bands glided between analyser frames. docs/design/koan-kit.md |
| `koan-kit: components/prism.rs` | The rainbow appearance as shaders on `Animated`: `Prism` (a layer over the wash flowing with the bands, the beat's edge pulse, the mirror ball, the disco over the page; `hear` takes the analyser and finds the beat, three a second at most), `Burst` (confetti, the sweep between records), `Sheen`, and the programs `PlayingBars::prism` and `Slider::prism` switch to. All decorative; nothing mounted while off |
| `koan-kit: components/answer.rs`, `slide.rs` | Controls answering what was done to them (`Answerer`, one `ANSWER` shader quad for a moment, nothing at rest or under Reduce Motion; `follow` for a change made elsewhere), and panels sliding in and out (`Slide`, `SplitView::shown`) |
| `koan-kit: components/` | The containers that own spacing (`Page`/`PageHeader`, `Toolbar`/`ToolbarButton`, `Form`/`FormSection`/`FormRow`, `TableRow` on `RowMetrics`, `Collection` of pills, tiles on `TileGrid` and rows, `SplitView`, `Menu` with sections, control and note rows, refilled in place while open, `Banner`, `SelectionBar`, `table_list` (a virtual list of `TableRow`s), `RowList` (one of rows measured as drawn, the queue's and a playlist's)), and the controls: buttons, toggles, segmented, slider, `Level` (a slider committed once, between two glyphs, holding the committed value until the device reports it), stepper, text and filter fields (the filter field with its clear mark and Escape), search suggestions, the `CommandBar`, tooltip, nav and list rows, badge, chip, pill, the `Heart`, `TransportControl`, empty state, the inspector and synced lines, detail groups, the artwork `Viewer`, the ensō `Placeholder` and `Spinner`, `Cover`, the drag chip, the fade-in, the scrolling page with the kōan scrollbar, the picker, the progress ring, `AvailabilityMark` (where a track's file is, or its download's ring read per frame), `DenseTable` for editors' tables, `ProgressRow` and `StatusBlock` for a sidebar's foot, `choice` menus for a choice with headings and actions, `QrCode` for links to scan, `SecondaryWindow` for the windows beside the main one, and `sheet`'s panel parts, its host for a sheet of the app's own (`present`, `close`) and question host; `FieldState` and `InputEvent` for a `TextField`'s owner. Pages declare content; `just gui-theme-leaks` holds koan-gui to it |
| `koan-kit: storybook.rs` | The storybook (`cargo run -p koan-kit`, `main.rs`): each component and container on its own page in each state, under controls for the appearance, the accent and a sample sleeve's wash, the text size, Reduce Motion and Show icons. `examples/storybook.rs` draws every page offscreen to PNGs, and `examples/spinner_frames.rs` and `slider_frames.rs` time the spinner and a slider's drag. The motion playground page (`storybook/playground.rs`) plays one keyframe track with any easing (a Bézier with draggable handles, a spring), plots its curve, scrubs it and gives the call to paste. |
| `dsp/model.rs` | `Dsp`, the one EQ model: the overview, writes through `act`, imports and their questions, AutoEQ, and the lane each edit's saves go through in order, each naming the filter the last one stored. A step of a drag plays without moving `version`; the last moves it |
| `dsp/graph.rs` | The response graph on the GPU and its editor: each curve's path tessellated once per change of curve, size or range around the level, and moved into place as painted; a drag previews one step at a time through the core and saves through the model's lane |
| `dsp/page.rs` | `EqPage`: a frame over the cached `EqBody` (a device's chain, its preset and the graph of the whole), with sheets from `dsp/sheet.rs` and the import questions from `dsp/prompts.rs` over it |
| `dsp/profile.rs`, `dsp/bands.rs` | A profile's sheet: the graph in a frame over the cached `ProfileBody` (role and targets, layers and groups, channels, keeping an edit) and the dense band table, whose rows save a figure only when its text changed and chain their saves |
| `dsp/manage.rs`, `dsp/stage.rs`, `dsp/flows.rs` | Manage EQ and recently deleted; choosing a stage's correction or EQ, a new EQ; AutoEQ, measurements and splitting a baked EQ |
| `dsp/window.rs`, `dsp/sheet.rs`, `dsp/menu.rs` | The one EQ window (Playback → EQ…, ⌥⌘E); the stack of sheets over the EQ pages, where a confirmation says what a change will do; the choice menus for device, preset, band type and target |
| `examples/eq.rs` | The EQ page against a real engine on a throwaway configuration: offscreen to a PNG, in a window, or `--measure` for a drag's frame times |
| `library/activity.rs` | The Library menu's tasks (rescan, force rescan, rebuild the index, sync) and the rows at the sidebar's foot showing each one running: progress from a `ProgressReporter` for scans started here and from the `Tasks` and `Sync` slices otherwise; scans can be stopped |
| `library/actions.rs` | What can be done with playable things from any library page: the context menu, the selection bar's verbs, a click on a sleeve; `Arrivals`, what the sidebar's queue and playlist rows answer |
| `library/chrome.rs`, `library/controls.rs` | What the pages share around their content (header with count and actions, filter panel, sort menus, select toggle, the pick's bar); the page's controls in the window's toolbar slot |
| `library/header.rs` | The header a record, artist or playlist page shares: cover, play and shuffle, title and detail, the lesser verbs |
| `library/info.rs` | A track's details over the page: tags, each source with its own fields, ReplayGain |
| `library/tokens.rs`, `library/format.rs` | The theme as the library pages draw it (`Paint`, text in a role); figures as the Mac app writes them |
| `library/stage.rs` | The library's part of the stage, for the shell: the page for an album, artist or track route, and a track's details over it |
| `library/nav.rs` | Each listing's scroll and anchor, and the track whose details are open, kept beside the navigator, which every move goes to |
| `library/covers.rs` | Library artwork: bytes on disk once per record (the platform cache, per configuration for any but the person's own), bitmaps per record and draw size, held to what the lists ask for and a screen more, each eviction dropping the GPU texture. A fetch lane and a decode lane take the most urgent job each time; lists replace their requests as they scroll (`request`). Decoded sleeves enter the atlas about 2 MB a frame (`admit`), drawn as their average colour meanwhile. Generations and `art_evictions` keep a reused row id from showing another record's sleeve |
| `library/albums.rs` | The album grid: a uniform list of rows of tiles, aiming the cover cache from the scroll's heading (the Mac grid's `aim`), scroll kept across visits and column changes |
| `library/tile.rs` | One record's tile, shared by the grid and the artist page: a cached view with one interactive element, telling its parts apart by position, its strings made once per listing |
| `library/rows.rs` | One track's row, shared by the track browser, record page and artist page, on the Mac's row metrics |
| `library/playlist.rs` | A playlist's page: its entries drawn by the queue's row parts (`queue::heading`, `queue::track`) and reordered by its rules (`queue::rows`), drops between rows, undo and redo, and rename, delete, shuffle, removal and export behind previews; `new_playlist`, which the sidebar and the context menus use |
| `library/artists.rs`, `tracks.rs` | The artist browser; the track browser, read a page at a time and swapped in once a reload covers the screen |
| `library/album_page.rs`, `artist_page.rs` | A record's page with disc runs; an artist's with the photo beside the name (its slot growing in when it arrives late), sections, features, appearances and the biography under "about" at the end |
| `library/drag.rs` | `PlayableDrag`, what every drag out of the library carries: `TrackSet`s for the engine, shared so a pick is built once. The queue and the sidebar's queue and playlist rows take it |
| `library/selection.rs` | Picking several things to play together, in tick order |
| `library/keys.rs` | The list keys (koan-kit's `list_keys`: arrows, ← → in a grid, Home/End, Page Up/Down, Return, Ctrl-Space, the menu key) for a library list or grid, through `KeyedList`: its length, how a range or a row is picked, revealing a row, what Return does, a grid's columns and rows (`below`, `line_below` for lines of different lengths), a grid's pick (`mark_range`), the page's `CursorSpot`, and what the menu key's menu acts on |
| `library/history.rs` | Play history: every play under its day, laid out off the main thread, in the library's track rows with the time as their lead and a mark for plays scrobbled in elsewhere; picked by play, forgotten and cleared behind previews |
| `library/shelf.rs` | Favourites, Recently played, Downloaded and the search's results: the head of each kind on the shelf in a koan-kit `Collection`, each heading opening the browser filtered to the shelf, one pick across the kinds |
| `library/scope.rs` | A shelf's "see all": the filter and order a browser takes in place of the shared ones (`Route::Shelved`), so the library browsers' own filter is left as it was |
| `search/mod.rs` | `Search`: the query and the search shelf's answer, each keystroke read off the main thread with older answers dropped by generation, read deeper while the search box is open (`deep`), which the results page cuts to its previews. An empty query forgets `Route::Search` |
| `search/field.rs` | The sidebar's search field: the query on koan-kit's `FilterButton`, whose click opens the search box |
| `search/bar.rs` | The search box over the window (koan-kit `CommandBar`), opened by ⌘K, `/`, ⇧⌘K and the sidebar's field: results as typed through `Search`, Return to play or open, ⌘Return next, ⌥Return to the queue, Tab to tick several, ⇧Return to the results page |
| `library/browse.rs` | The shared filter and search, each browser's sort, kept in `gui-state.json` in the state directory by a writer thread |
| `library/feed.rs` | What the library pages read, derived from `mirror.rs`'s slices (library version, record and track playing, downloads by record, connection), with the favourites and playlists read from the engine as the library moves |
| `library/bench.rs` | What `benches/album_grid.rs` counts beside GPUI's frame timings: blank, placeholder and stand-in tiles, and how long tiles stayed blank. The bench scrolls the grid through GPUI's harness and headless renderer, nothing on screen |

`just gui-library DIR [ALBUMS] [TRACKS]` generates and scans a large throwaway library; `KOAN_CONFIG_DIR=DIR/config cargo bench -p koan-gui --bench album_grid` scrolls the album grid over it.

`settings/` is the Settings window, the Mac app's `SettingsView` in the kōan look: `model.rs` is a view onto `config.toml` (shared with the CLI and TUI) and what Settings does to the engine, `panes/` one view per pane, `form.rs` the panes' layout over koan-kit's `Form`, `sheet.rs` the one sheet at a time where every write that changes data shows what it will change, `sharing.rs` this device serving its library (`share.enabled`), and `qr.rs` invite codes drawn as quads. `examples/settings_shot.rs` renders every pane.

`platform/` is where the app meets the operating system. Nothing in it blocks the UI thread: engine calls are awaited from `cx.spawn`, file work runs on the background executor, the log is written by a thread of its own.

| Module | What |
|--------|------|
| `platform/about.rs` | About kōan (the standard panel on macOS, a window elsewhere), and the name and icon a build outside a bundle gives itself on macOS. The build is the commit, from `build.rs`, or `KOAN_BUILD` |
| `platform/dock.rs` | The Dock's menu on macOS, rebuilt from the Playback and Devices slices only when what it shows changes |
| `platform/mod.rs` | `init`: everything that runs for the app's life, quitting as the Mac app quits, and what files and links from outside become (`Links` for the shell). It restores the desktop app's own saved session before taking launch files, so they land after it, and saves it on quit; the engine refuses a save that would replace a session not yet read back (`SessionGate` in koan-runtime) |
| `platform/actions.rs` | Every shortcut, from one table: the key bindings, the macOS menu bar's items and the shortcuts sheet. Modifier shortcuts in the `koan` context, so a text field's own keys win; bare keys in `koan && !Input`, never a menu's key equivalent. Engine-only commands handled here; the rest are the shell's and the pages' to handle |
| `platform/menus.rs` | The app's menus, in the Mac app's order, described once (`menus`): macOS's menu bar (`set`), and on Linux, which has no global menu bar, the same menus drawn in the window behind the toolbar's kōan button (`in_window`), each item with its keys |
| `platform/residency.rs` | Keeping running with the window closed (`devices.keep_running`): the quit rule, leaving the Dock for the menu bar on macOS, reopening the window; opening at login, asked of the system (`SMAppService` on macOS, an XDG autostart entry on Linux) |
| `platform/status_item.rs` | The macOS menu bar item while kept running: the ensō, and a menu of what is playing, play and pause, next, open and quit, rebuilt only when it changes |
| `platform/startup_error.rs` | The window shown when the engine cannot open, saying what failed and what to do in the engine's words, as the Mac's `StartupErrorView`; closing it quits, and another launch brings it to the front |
| `platform/shortcuts_sheet.rs` | What `?` and ⌘/ show |
| `platform/media.rs` | souvlaki: MPRIS on Linux, Now Playing on macOS, cover included, from a state stream of its own; `Playhead`, the anchor the skip shortcuts read |
| `platform/drop.rs` | Files from outside — drops, the command line, a second launch — indexed where they lie and queued, or added to a playlist. `Imports` for the shell's activity row and error line |
| `platform/instance.rs` | One app per configuration directory: a lock beside the device's, a Unix socket a second launch hands its paths to |
| `platform/runtime_files.rs` | The app's runtime files (window and browse state, the desktop device's id and devices, locks, Now Playing's covers) outside the config directory, in `koan_core::places`; files an earlier version left there are moved on start-up, once |
| `platform/logging.rs` | `koan.log`, in every front end's format, and stderr |

The workspace's `vendor/` holds patched copies of GPUI (`[patch.crates-io]` in the root `Cargo.toml`): its macOS and Linux backends, so an idle window stops waking at every display refresh (#1609), its core and renderers, for shader quads whose uniforms are read at present (#1641), and gpui-kit's gpui-base, whose text caret is such a quad so its blink redraws nothing (#1786); every change is marked `KOAN:`, and `vendor/README.md` says how to move them to a new GPUI release.

`LinkIdentity::desktop` (koan-core) and `KoanEngine::open_desktop` (koan-runtime) make it a device of its own beside the Mac app and the terminal UI on one config directory. `just gui-run`, `gui-build` and `gui-bundle` (a signed `.app` and DMG on macOS, an AppImage and `.deb` on Linux) build and package it; `packaging/` holds the Linux desktop entry and the app icons.

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
| `KoanIOS/PushDelegate.swift` | Push: registers for a token and hands it to the engine for the relay; wakes to link on a background push; sends a tapped notification's claim up the link. `KoanIOS/AppAttest.swift` attests for the relay |
| `Support/ActivityModel.swift` | The one place that knows what koan is busy with. Each task declares what it holds — files on disk, local rows, remote rows, downloads — and a new one is disabled only where those overlap |
| `Support/Pairing.swift` | Approving a device that is waiting to sign in: a `koan.rocks/pair/` link arriving through `AppState.open(url:)`, or a code typed in Settings, asks the server for the device's name and then the person, as invites do |
| `Support/SettingsModel.swift` | Settings state over `config.toml`. Commits on edit, re-reads on focus |
| `Support/SharingModel.swift` | The Mac serving its own library: the account it is shared under, starting at launch when `share.enabled`, and why a start failed. `Views/ShareSettings.swift` is Settings → Server's mode choice and the line, there and at the foot of the sidebar, saying what the Mac is doing |
| `Support/KoanTheme.swift` | The kōan theme: tokens, the record accent (OKLCH, cached per colour), motion, and the roles views use instead of colours, fonts, corners and materials. Each role draws the platform's look unchanged when the theme is off. Spec: `docs/design/koan-theme.md`; `just theme-leaks` finds what bypasses it |
| `Support/KoanThemeSheet.swift` | Every component of the theme on one page, for the evidence renderer |
| `Support/Icons.swift` | `Icon.*`, every icon the apps draw, named once by its SF Symbol, and `Image(koan:)`, `Label(_:koan:)`, `Button(_:koan:)` for the places that take only an image |
| `Support/KoanGlyph.swift` | The kōan icon set: glyphs compiled from `Resources/Icons/*.svg` by `just icons` into `KoanGlyphs.swift`, the stroke rule, drawing. `KoanIcon` and `Symbol.image` draw them in the theme |
| `Support/EngineMirror.swift` | The engine's state as SwiftUI sees it. `Observable` by hand: one property per slice, invalidated only where a slice actually moved |
| `Support/PlayerModel.swift` | What the app *does* to the player — commands, and the little that is genuinely local. Reads everything through the mirror |
| `Support/Navigator.swift` | Where the app is: one page, the linear history of pages visited, and a cursor. No `NavigationStack` — koan navigates like a browser, any page from any page |
| `Support/LibraryModel.swift` | Browse state. Holds what the section on screen is showing and nothing else — narrowing and sorting happen in SQL, listings arrive whole. Follows the navigator; never moves it |
| `Support/PlayableSelection.swift` | Things picked out of a page to play or queue together — the album grid, an artist's records, search results of every kind. A mode, because a click already plays or navigates. Held in tick order, so a pick can span several filters or queries |
| `Support/CoverArtCache.swift` | Album-keyed art cache: bytes once per record on disk, bitmaps per record and draw size in a bounded `NSCache`. Deliberately off the main actor — see the note there. Each miss is an HTTP round trip on remote libraries |
| `Support/ImageWork.swift` | The two lanes image work runs in, neither of them the cooperative pool: a wide one for blocking file reads, a bounded one for decoding that runs what is on screen first and drops what nobody is waiting for |
| `Support/Platform.swift` | The few types AppKit and UIKit disagree about. `KoanApp`, `RootView`, `Hotkeys`, `TextFocus`, `EditCommands`, `MenuShortcuts` and `ShortcutsSheet` are the macOS shell and have no iOS counterpart; everything else builds for both |
| `Views/DriftingWash.swift` | The window's wash, as Core Animation. Drift, blur and dissolve belong to the compositor; nothing here costs a main-thread frame |
| `Support/FrameTimer.swift` | Times a tap against the display link, so the region after a body evaluation — layout, the commit, the render server — is measurable at all. See CONTRIBUTING |
| `Support/TransferMeter.swift` | Download progress at the display's rate: a display link, alive only while a transfer runs and something on screen draws it, reading byte counts per frame and handing them to rings and bars as layer geometry |
| `Support/PlayingLevels.swift` | One analyser subscription for every playing indicator on screen, handing each frame straight to the bars as layer geometry — nothing observable, nothing SwiftUI re-runs. Reads the stream only while a bar is attached, which is what lets the analyser park |
| `Views/QueueView.swift` | The main stage — album-grouped queue, drag reorder, multi-select. On the Mac a `KoanTable` of `QueueTableRow`s. Never torn down: `StageView` keeps it mounted behind other pages, so its place and its playing row survive a visit elsewhere. The album and artist browsers are rebuilt on each visit and restore their scroll position; kept mounted, they made every page switch lay them out |
| `Views/AlbumCollection.swift` | The Mac's album grid: `NSCollectionView`, tiles of layers and labels. SwiftUI's grid cost 22–27 ms per scroll step at 4K; this one 11–12. Behaviour mirrors `AlbumGridCell`, which iOS and the artist page keep. AppKit controls are SwiftUI graphs on macOS 26, so a tile makes its few only while showing them. Fetches covers ahead of the scroll from its speed and a flick's landing spot, and makes the nearest rows' tiles off screen |
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

### apps/android (`apps/android/app/src/main/kotlin/cc/blit/koan/`)

Gradle's `koanFfi` task builds koan-ffi with cargo-ndk into `jniLibs` and generates the Kotlin bindings (package `cc.blit.koan.ffi`, from `crates/koan-ffi/uniffi.toml`) before every build; neither is checked in. The SDK comes from `ANDROID_HOME` or `local.properties`, the JDK and cargo-ndk from mise.

| Module | What |
|--------|------|
| `KoanApp.kt` | The `Application`: points the core's config directory at `filesDir`, hands the core the app's `Context`, opens the one engine, builds Coil's loader |
| `engine/EngineMirror.kt` | The engine's state slices as `StateFlow`s, read by one loop off the main thread; queue patches applied to their base. `PlayerModel.kt` sends the commands |
| `playback/KoanPlayer.kt` | The engine as a Media3 `SimpleBasePlayer`: reports queue and playhead, forwards the session's commands. Nothing plays through Media3 |
| `playback/PlaybackService.kt` | `MediaSessionService`: background playback and the notification, audio focus, pause on becoming noisy, output rebuilt on a route change |
| `playback/BitPerfect.kt` | `BitPerfectHost`: a USB DAC's bit-perfect mixer attributes for the engine to choose from, held and released through `AudioManager`; the person's on/off preference |
| `art/Art.kt` | Covers for Coil, fetched from the engine, bytes kept on disk, dropped on `art_evictions` |
| `ui/AppShell.kt` | Sign-in, then four tabs (Queue, Library, Search, Settings) on Navigation 3, each with its own back stack; the mini player; Now Playing over the tabs |
| `ui/<feature>/` | One package per area, each registering its pages through an `…Entries()` function. See `apps/android/README.md` |

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
| `push.rs` | Push to the iOS app through kōan's push relay (`koan-relay-client`), by the handles apps give. A background push wakes a suspended app to link; a play request becomes a notification whose claim (`clients::claims`) the app redeems on tap; Live Activity updates |
| `pair.rs` | Pairings waiting to be approved, in memory: id, Crockford code, device name, ten-minute lapse, the socket route that holds the device, and `settle`, which mints the approver's key and sends it down. `/rest/koanPairInfo` and `/rest/koanPairApprove` in `subsonic.rs` and the web UI's `/pair` (`ui/pair.rs`) settle through it |
| `transcode.rs` | Subsonic `stream` transcoding: whether a request gets the original or an `ffmpeg` Opus, MP3 or AAC encode, and running it |
| `icon.rs`, `icon/glyphs.rs` | The kōan icon set for the web UI and share page: `glyphs.rs` is generated by `just icons` from the apps' SVGs; `Glyph::svg` draws one inline on the theme's stroke rule |
| `share.rs` | Public share pages and their audio, answering for a share's own tracks only |
| `../styles/` | Tailwind sources for `assets/ui.css` and `assets/share.css`, on the theme koan.rocks uses (`site/src/theme.css`). The pages are styled with utilities in the templates; these hold element defaults and the rules for classes the scripts toggle or build (`playing`, `busy`, `missing`, the queue's rows). Quote every `class` attribute: Tailwind does not read an unquoted one. Run `just css` after changing either; the compiled files are committed because the crate embeds them |
| `../scripts/` | TypeScript sources for the web UI's `ui.js` and `player.js` and the share page's `share.js`. `build.rs` strips their types with oxc into `OUT_DIR`, which the crate embeds, so nothing compiled is committed and `cargo build` needs no JavaScript toolchain; `just js` typechecks them with `tsc`. Erasable syntax only: no enums, namespaces or parameter properties. ES modules: `ui.ts` and `share.ts` import the `Player` class from `koan/player`, which each page's import map names at the player's versioned URL, and the CSP allows the map by its hash (`share::ImportMap`). `koan.d.ts` declares only browser APIs TypeScript's DOM types lack |
| `../templates/` | askama templates for the web UI and share page: `pages/` holds page bodies, `shell.html` and `signin.html` the frames around them, and one directory per area its fragments. Output is escaped unless a template says `\|safe`, which is for markup the server built. Shared class strings stay Rust constants that templates name by path (`crate::ui::pages::SUB`) |
| `ui/` | Web UI: server-rendered pages + Datastar, cookie-session gate, sign-in/resume/renew/sign-out, stream and cover routes. `scripts/player.ts` is the browser player both it and the share page use. `ui/oauth.rs` is the OAuth 2.1 authorization server for `/mcp`: discovery, stateless registration, consent, PKCE token exchange; `ui/connect.rs` the page explaining how to connect an assistant; `ui/scrobbling.rs` the page connecting an account to ListenBrainz or Last.fm |

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

The kōan GUI app's next work is tracked in the pinned issue #1697. `.claude/plans/` holds research for features not yet built, chiefly tag editing (plan 04: vimv-style TSV + `$EDITOR` first) and gapless encoder trim on lossy codecs (plan 10). Its README's status table predates much of what has shipped (DSP in the apps, artist metadata); trust the code over it.

# 06: Decoupled Backends — Trait-Based Subsystem Architecture

## Summary

Every major subsystem in koan is currently hard-wired to a single platform/implementation: CoreAudio for output, macOS Keychain for credentials, rusqlite for storage, Subsonic for remote. This plan defines the trait boundaries, feature flag structure, and migration path to make each subsystem pluggable — the foundational change that enables Linux support.

The core insight: **not every subsystem needs the same dispatch strategy**. Audio needs static dispatch (performance-critical, determined at compile time). Credentials and remote can use trait objects (configured at runtime). Database should stay SQLite — abstracting it buys nothing.

---

## Current Coupling Analysis

### Audio Engine (`koan-core/src/audio/engine.rs`, `device.rs`)
- **Depth: Deep.** Raw `coreaudio_sys` FFI. `AudioDeviceID` (a `u32` typedef) leaks through the entire player. `device::default_output_device()` returns platform-specific types. The render callback is a C function pointer with CoreAudio's exact `AudioBufferList` signature.
- **Coupling surface**: `Player::start_playback()` calls `device::default_output_device()`, `device::get_device_sample_rate()`, `device::set_device_sample_rate()`, then constructs `engine::AudioEngine::new(device_id, ...)`.
- **What leaks**: `AudioDeviceID` type, device enumeration API shape, sample rate get/set as separate functions.

### Credentials (`koan-core/src/credentials.rs`)
- **Depth: Shallow.** Three free functions (`store_password`, `get_password`, `delete_password`), each wrapping one `security_framework` call. Only consumed by the `login` CLI command.
- **Coupling surface**: Minimal. Replace these three functions and you're done.

### Remote (`koan-core/src/remote/client.rs`, `sync.rs`)
- **Depth: Medium.** `SubsonicClient` exposes Subsonic-specific types (`SubsonicArtist`, `SubsonicAlbum`, `SubsonicSong`). `sync::sync_library()` transforms these into `TrackMeta` for the DB. The sync module is the adapter — it already converts protocol-specific types to generic ones.
- **Coupling surface**: `sync.rs` imports `SubsonicClient` and all its response types. The TUI doesn't touch the client directly.

### Database (`koan-core/src/db/`)
- **Depth: Deep but irrelevant.** 450+ lines of raw SQL with SQLite-specific features (FTS5, `COALESCE`, `last_insert_rowid`). But SQLite is already the right choice for an embedded music player. There's no realistic scenario where you'd swap it for Postgres or sled.
- **Recommendation: Don't abstract.** Leave it as-is. The "pluggable" value is zero.

### Media Keys (`koan-music/src/media_keys.rs`)
- **Depth: Medium.** Uses `souvlaki` which is already cross-platform. The macOS-specific piece is `pump_run_loop()` which calls `CFRunLoopRunInMode`. This is already behind `#[cfg(target_os)]`.
- **Coupling surface**: `core-foundation` dependency in koan-music. The `pump_run_loop()` pattern.

---

## Trait Designs

### 1. Audio Backend

The hardest one. CoreAudio uses a **push model** (OS calls your render callback when it needs samples). ALSA uses a **pull model** (you write samples to the device). PipeWire can do either. The ring buffer architecture actually makes this tractable — the backend just needs to drain the consumer.

```rust
// crates/koan-core/src/audio/backend.rs

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// Opaque device identifier — each backend defines its own inner type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

/// Platform-agnostic audio device info.
#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    pub id: DeviceId,
    pub name: String,
    pub sample_rates: Vec<f64>,
}

/// Errors from audio backend operations.
#[derive(Debug, thiserror::Error)]
pub enum AudioBackendError {
    #[error("no output devices found")]
    NoDevices,
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("unsupported sample rate: {0}")]
    UnsupportedSampleRate(f64),
    #[error("backend error: {0}")]
    Backend(String),
}

/// Configuration for creating an audio output stream.
pub struct OutputConfig {
    pub device: DeviceId,
    pub sample_rate: f64,
    pub channels: u32,
    pub consumer: rtrb::Consumer<f32>,
    pub samples_played: Arc<AtomicU64>,
}

/// A running audio output stream. Stops on drop.
pub trait AudioStream: Send {
    fn start(&self) -> Result<(), AudioBackendError>;
    fn stop(&self) -> Result<(), AudioBackendError>;
    fn is_running(&self) -> bool;
}

/// The main backend trait. One implementation per platform.
pub trait AudioBackend: Send + Sync {
    /// List available output devices.
    fn list_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioBackendError>;

    /// Get the default output device.
    fn default_device(&self) -> Result<DeviceId, AudioBackendError>;

    /// Get current sample rate of a device.
    fn device_sample_rate(&self, device: &DeviceId) -> Result<f64, AudioBackendError>;

    /// Set device sample rate (for bit-perfect matching). May fail if
    /// the backend doesn't support runtime sample rate changes.
    fn set_device_sample_rate(
        &self,
        device: &DeviceId,
        rate: f64,
    ) -> Result<(), AudioBackendError>;

    /// Create and return an output stream. The stream drains the ring
    /// buffer consumer and increments samples_played.
    fn open_output(
        &self,
        config: OutputConfig,
    ) -> Result<Box<dyn AudioStream>, AudioBackendError>;
}
```

**Why not cpal?** cpal doesn't support:
- Explicit device sample rate switching (critical for bit-perfect)
- Exclusive mode on most backends
- Direct control over the buffer callback pattern

cpal is designed for "give me audio I/O and don't make me think about it" — great for games, terrible for audiophile playback. We need low-level control. The trait above lets us wrap `coreaudio-sys` (macOS), `alsa-rs` (Linux), and potentially `wasapi` (Windows) directly.

**Push vs Pull unification:** The `OutputConfig` hands the backend a ring buffer consumer. For push-model backends (CoreAudio), the render callback drains it. For pull-model backends (ALSA), a dedicated thread writes in a loop. Either way, the contract is: drain `consumer`, increment `samples_played`. The trait consumer doesn't care.

### 2. Credential Backend

```rust
// crates/koan-core/src/credentials.rs (refactored)

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("credential not found")]
    NotFound,
    #[error("backend error: {0}")]
    Backend(String),
    #[error("invalid utf-8 in credential")]
    InvalidUtf8,
}

pub trait CredentialStore: Send + Sync {
    fn store(&self, account: &str, secret: &str) -> Result<(), CredentialError>;
    fn get(&self, account: &str) -> Result<String, CredentialError>;
    fn delete(&self, account: &str) -> Result<(), CredentialError>;
}
```

**Implementations:**
- `KeychainStore` — current macOS `security-framework` code, wrapped.
- `KeyringStore` — uses the `keyring` crate (v4), which already abstracts macOS Keychain, Linux Secret Service/kwallet, and Windows Credential Manager. This is the **recommended default** for cross-platform. It does everything we need and is actively maintained.
- `FileStore` — encrypted file fallback (`age` crate or `chacha20poly1305`). For headless Linux boxes without a secret service.
- `PlaintextStore` — config.local.toml password field (already exists). Print a warning.

**Selection:** Runtime, from config:
```toml
[credentials]
backend = "keyring"  # "keychain" | "keyring" | "file" | "plaintext"
```

The `keyring` crate (v4.0.0-rc.3, nearly stable) is the pragmatic choice here. It wraps platform-specific backends and exposes a simple get/set/delete API. Using it means we don't need to write `KeychainStore`, `SecretServiceStore`, `KwalletStore` separately — `keyring` does that already. Our trait still has value as a seam for testing and for the file/plaintext fallbacks.

### 3. Remote Source Backend

```rust
// crates/koan-core/src/remote/mod.rs (refactored)

use crate::db::queries::TrackMeta;

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("api error: {0}")]
    Api(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Summary of a sync operation.
#[derive(Debug, Default)]
pub struct SyncResult {
    pub artists_synced: usize,
    pub albums_synced: usize,
    pub tracks_synced: usize,
}

/// A remote music source. Implementations handle protocol-specific
/// details and translate to koan's internal types.
pub trait RemoteSource: Send + Sync {
    /// Verify connection and credentials.
    fn ping(&self) -> Result<(), RemoteError>;

    /// Pull the full library, calling `on_track` for each discovered track.
    /// The implementation handles pagination, rate limiting, etc.
    fn sync_library(
        &self,
        on_track: &mut dyn FnMut(TrackMeta),
    ) -> Result<SyncResult, RemoteError>;

    /// Build a streaming URL for a track by its remote ID.
    fn stream_url(&self, remote_id: &str) -> Result<String, RemoteError>;

    /// Download a track to a local path.
    fn download(
        &self,
        remote_id: &str,
        dest: &std::path::Path,
        on_progress: &dyn Fn(u64, u64),
    ) -> Result<(), RemoteError>;

    /// Report a play (scrobble).
    fn scrobble(&self, remote_id: &str) -> Result<(), RemoteError>;

    /// Search the remote library.
    fn search(&self, query: &str) -> Result<Vec<TrackMeta>, RemoteError>;
}
```

**Why `on_track` callback instead of returning `Vec<TrackMeta>`?** The sync can yield thousands of tracks. Streaming them via callback lets the caller write to DB in batches without holding the entire dataset in memory. The existing `sync_library` already works page-by-page — this formalizes that.

**Implementations:**
- `SubsonicSource` — current client, wrapping existing code.
- `JellyfinSource` — Jellyfin has a Subsonic-compatible endpoint, but also its own richer API. The `jellyfin-sdk` crate exists but is early-stage. Could implement against the REST API directly.
- `PlexSource` — Plex has a well-documented XML/JSON API. Lower priority.
- `FunkwhaleSource` — Funkwhale implements the Subsonic API, so `SubsonicSource` works out of the box.

**Note:** `SubsonicClient`'s response types (`SubsonicArtist`, etc.) become private to the subsonic implementation. The trait boundary ensures they never leak into the rest of the codebase.

---

## Backend Implementation Matrix

| Subsystem    | Current          | Trait?   | Dispatch    | Priority  | Complexity |
|-------------|-----------------|----------|-------------|-----------|------------|
| Audio       | CoreAudio       | Yes      | Static/cfg  | P0        | High       |
| Credentials | macOS Keychain  | Yes      | Runtime     | P1        | Low        |
| Remote      | Subsonic        | Yes      | Runtime     | P2        | Medium     |
| Media Keys  | souvlaki        | No*      | N/A         | P3        | Low        |
| Database    | SQLite          | No       | N/A         | Never     | N/A        |
| Tag Reading | lofty           | No       | N/A         | Never     | N/A        |

*souvlaki is already cross-platform. Only the `pump_run_loop()` CFRunLoop call needs `#[cfg]`.

### Planned Backend Implementations

**Audio:**
| Backend     | Platform | Crate            | Bit-Perfect | Sample Rate Switch | Status    |
|------------|----------|------------------|-------------|-------------------|-----------|
| CoreAudio  | macOS    | `coreaudio-sys`  | Yes         | Yes               | Exists    |
| ALSA       | Linux    | `alsa`           | Yes*        | Yes*              | Needed    |
| PipeWire   | Linux    | `pipewire`       | Depends     | Depends           | Future    |
| WASAPI     | Windows  | `windows` / raw  | Exclusive   | Exclusive mode    | Future    |

*ALSA bit-perfect requires `hw:` device (bypass dmix). Sample rate switching works on `hw:` devices.

**Credentials:**
| Backend    | Platform     | Crate                | Status    |
|-----------|-------------|---------------------|-----------|
| Keychain  | macOS       | `security-framework` | Exists    |
| Keyring   | All         | `keyring` v4         | Easy add  |
| File      | All         | `age`/`chacha20`     | Future    |
| Plaintext | All         | N/A (config.local)   | Exists    |

**Remote:**
| Protocol   | Crate/Method       | Status    |
|-----------|-------------------|-----------|
| Subsonic  | Direct HTTP       | Exists    |
| Jellyfin  | Direct HTTP / SDK | Future    |
| Plex      | Direct HTTP       | Future    |

---

## Feature Flag Structure

```toml
# crates/koan-core/Cargo.toml

[features]
default = ["audio-coreaudio", "cred-keychain"]

# Audio backends (exactly one required at build time)
audio-coreaudio = ["dep:coreaudio-sys"]
audio-alsa = ["dep:alsa"]
audio-pipewire = ["dep:pipewire"]

# Credential backends (can enable multiple; selected at runtime)
cred-keychain = ["dep:security-framework"]
cred-keyring = ["dep:keyring"]

# Remote sources (can enable multiple; selected at runtime via config)
remote-subsonic = ["dep:md5"]  # md5 only needed for subsonic auth
remote-jellyfin = []

[dependencies]
# Audio — all optional, gated by feature
coreaudio-sys = { version = "0.2", optional = true }
alsa = { version = "0.9", optional = true }
pipewire = { version = "0.8", optional = true }

# Credentials — all optional
security-framework = { version = "3.5", optional = true }
keyring = { version = "4", optional = true }

# Remote — md5 only needed for subsonic token auth
md5 = { version = "0.8", optional = true }

# Always needed (ring buffer, decode, db, etc.)
rtrb = "0.3"
symphonia = { version = "0.5", features = ["all"] }
rusqlite = { version = "0.38", features = ["bundled-full"] }
# ... rest unchanged
```

**Platform defaults via Cargo's target-specific features:**
```toml
# Workspace Cargo.toml or build script
# Unfortunately Cargo doesn't support `[target.'cfg(...)'.features]` yet.
# Use a build.rs or .cargo/config.toml approach:

# .cargo/config.toml
[target.'cfg(target_os = "macos")']
rustflags = []  # features selected in the default profile

# Alternative: use cfg_aliases in build.rs to set cfg flags,
# then use #[cfg] in the code to pick the backend.
```

In practice, the cleanest approach is:
```toml
[features]
default = []
macos = ["audio-coreaudio", "cred-keychain"]
linux = ["audio-alsa", "cred-keyring"]
```

And the binary crate selects:
```toml
# crates/koan-music/Cargo.toml
[target.'cfg(target_os = "macos")'.dependencies]
koan-core = { path = "../koan-core", features = ["macos"] }

[target.'cfg(target_os = "linux")'.dependencies]
koan-core = { path = "../koan-core", features = ["linux"] }
```

### "At Least One Backend" Constraint

Rust's feature system can't enforce "at least one of these features". Handle it with a compile-time check:

```rust
// crates/koan-core/src/audio/mod.rs
#[cfg(not(any(
    feature = "audio-coreaudio",
    feature = "audio-alsa",
    feature = "audio-pipewire"
)))]
compile_error!("At least one audio backend feature must be enabled");
```

---

## Migration Strategy

### Phase 0: Preparation (no behavioral changes)
1. **Extract `AudioDeviceID` from player.** The `Player::start_playback()` method currently calls `device::` functions directly with `AudioDeviceID`. Refactor so `Player` holds an abstract device reference (string name or `DeviceId`), not a CoreAudio `u32`.
2. **Move response types behind the sync boundary.** `SubsonicArtist`, `SubsonicAlbum`, etc. should not be `pub` from `koan-core`. They're internal to the subsonic implementation. The sync module already converts them to `TrackMeta` — just tighten visibility.
3. **Gate `pump_run_loop()` properly.** Already has `#[cfg(target_os)]` — just ensure it compiles cleanly with no macOS imports when targeting Linux.

### Phase 1: Audio Backend Trait
1. Define `AudioBackend`, `AudioStream`, `AudioDeviceInfo` traits in `audio/backend.rs`.
2. Implement `CoreAudioBackend` wrapping existing `engine.rs` + `device.rs` code.
3. Refactor `Player` to hold `Box<dyn AudioBackend>` (or generic `P: AudioBackend`).
   - Generic is better — avoids vtable overhead in the hot path. But `AudioBackend` methods aren't called per-sample (only on start/stop/device-change), so `dyn` is fine.
   - **Decision: `Box<dyn AudioBackend>`** — simpler, no generic propagation through Player → SharedPlayerState → the rest of the world.
4. Add `#[cfg(feature = "audio-coreaudio")]` around the CoreAudio implementation.
5. Write a stub `AudioBackend` for testing (plays silence, useful for CI on Linux).

### Phase 2: Credential Backend Trait
1. Define `CredentialStore` trait.
2. Wrap existing `security-framework` code as `KeychainStore`.
3. Add `KeyringStore` using the `keyring` crate — this immediately gives Linux + Windows support.
4. Gate with features.
5. Selection at runtime from config (defaulting based on compile-time features).

### Phase 3: Remote Source Trait
1. Define `RemoteSource` trait.
2. Refactor `SubsonicClient` to implement it.
3. Refactor `sync::sync_library` to accept `&dyn RemoteSource` instead of `&SubsonicClient`.
4. Make `SubsonicArtist`/`SubsonicAlbum`/`SubsonicSong` private to the subsonic module.
5. Selection at runtime from config (`remote.type = "subsonic"`).

### Phase 4: Feature Flags & CI
1. Wire up Cargo features.
2. Add Linux CI target (GitHub Actions `ubuntu-latest`).
3. Cross-compile check: `cargo check --features linux --target x86_64-unknown-linux-gnu`.
4. Release pipeline: build macOS + Linux binaries.

---

## Complexity Assessment

### Audio Backend — HIGH
- **Reason:** The CoreAudio code is 270 lines of unsafe FFI with a real-time render callback. The ALSA equivalent is different in kind — it's a blocking write loop, not a callback. The trait must unify push and pull models without compromising latency.
- **Risk:** Getting the ALSA backend right (proper period/buffer sizing, MMAP mode, recovery from xruns) is a project in itself. This is where most of the actual engineering time goes.
- **Estimated effort:** 2-3 days for the trait + CoreAudio refactor. 3-5 days for a production-quality ALSA backend. PipeWire adds another 2-3 days.
- **Testing:** The `NullBackend` (plays silence, increments counter) unblocks CI and tests on all platforms.

### Credential Backend — LOW
- **Reason:** Three functions. The trait is trivial. The `keyring` crate already does the hard work.
- **Estimated effort:** Half a day. Most of it is testing the `keyring` integration on Linux.

### Remote Source — MEDIUM
- **Reason:** The trait surface is straightforward, but `sync_library` has complex batching/transaction logic. The refactor to use a callback pattern instead of direct DB writes needs care.
- **Risk:** Jellyfin/Plex API differences might force trait changes. Start with just Subsonic behind the trait, add others later.
- **Estimated effort:** 1-2 days for the trait + Subsonic refactor. Each new remote backend is 2-4 days.

### Media Keys — LOW
- **Reason:** souvlaki already handles it. Just need to ensure `pump_run_loop()` compiles out on Linux and the `core-foundation` dependency is gated.
- **Estimated effort:** A few hours. Mostly Cargo.toml changes.

### Database — NOT DOING
- **Reason:** SQLite is the correct embedded DB. FTS5, WAL mode, single-file — there's nothing to gain from abstraction. Every alternative (sled, redb) would be worse.

---

## Recommended Implementation Order

1. **Phase 0** — Decouple `AudioDeviceID` from `Player`, tighten visibility on Subsonic types. No functional changes, all tests pass. *Do this first on `main` before any backend work.*

2. **Phase 2: Credentials** — Easiest win. Unblocks Linux login flow. Drop-in `keyring` crate. Can be done in a single PR.

3. **Phase 1: Audio** — The big one. Define trait, wrap CoreAudio, add NullBackend. This PR doesn't add Linux audio yet — it just makes the architecture ready. Then ALSA backend in a follow-up PR.

4. **Phase 3: Remote** — Least urgent. Subsonic works. Trait-ify it when someone actually wants Jellyfin support.

5. **Phase 4: CI/Feature Flags** — Wire it all together. Linux CI, release builds, cross-compile.

The credential backend first might seem odd, but it's the "small PR that proves the pattern works" — establishes the trait + feature flag conventions that the bigger audio refactor follows.

---

## Open Questions

1. **Player generics vs dyn dispatch:** `Player<B: AudioBackend>` would propagate the generic everywhere. `Box<dyn AudioBackend>` is simpler but adds a vtable lookup on each `open_output` call. Since `open_output` is called once per track (not per sample), dyn dispatch is the clear winner. The `AudioStream` returned from `open_output` is already `Box<dyn AudioStream>`.

2. **Ring buffer ownership:** Currently the player creates the ring buffer and passes the consumer to the engine. This works for both push and pull models — the backend gets a consumer it drains however it wants. No changes needed.

3. **Sample rate switching on Linux:** ALSA can switch sample rates on `hw:` devices but not on `plughw:` or `default`. PipeWire requires a different approach. The trait's `set_device_sample_rate` returning `Result` handles this — backends that can't switch return `Err`, and the player falls back to the device's native rate (with resampling in the future).

4. **ALSA blocking write thread:** The CoreAudio backend doesn't need its own thread (the OS calls the render callback on its thread). ALSA will need a dedicated thread running a `write` loop. This thread should live inside the `AlsaStream` implementation, spawned on `start()` and joined on `stop()`/`drop()`.

5. **Config schema for backend selection:** Audio backend is compile-time (feature flags). Credential and remote backends are runtime (config.toml). Should config.toml have a `[backends]` section or per-subsystem keys? Per-subsystem is cleaner:
   ```toml
   [credentials]
   backend = "keyring"

   [remote]
   type = "subsonic"
   url = "..."
   ```

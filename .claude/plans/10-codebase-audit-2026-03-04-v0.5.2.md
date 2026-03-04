# 10 — Codebase Audit v0.5.2

**Date:** 2026-03-04
**Version:** 0.5.2
**Scope:** Full codebase — code quality, dead code, smells, bugs, architecture, security, performance, dependencies, file sizes, test coverage

---

## Executive Summary

koan is a well-engineered Rust codebase. Clippy is fully clean, there are no TODO/FIXME markers, no commented-out code, no hardcoded secrets, and all SQL queries are parameterized. The main concerns are: 2 HIGH security issues (credential handling), performance bottlenecks in the render loop (string clones), and `app.rs` at 2,319 lines needing decomposition. Test coverage has gaps in integration testing and some AI-generated tests are low-value.

---

## 1. Security

### HIGH

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| S1 | **Plaintext password in config without restrictive permissions** — `config.local.toml` written with default 0644 permissions (world-readable) | `koan-core/src/config.rs:220-228` | Add `chmod 0o600` after write on unix; consider using existing Keychain integration in `credentials.rs` |
| S2 | **Auth credentials in stream URLs** — username, token, salt visible in query strings; could leak to logs/proxies | `koan-core/src/remote/client.rs:133-141` | Ensure stream URLs are never logged; consider downloading via `reqwest` instead of raw URL construction |

### MEDIUM

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| S3 | **MD5 auth token** — Subsonic protocol uses `MD5(password + salt)`, trivially brute-forceable | `client.rs:45` | Protocol limitation; document HTTPS requirement; prefer API key auth if server supports it |
| S4 | **Predictable temp file for cover art** — fixed path `koan-now-playing-cover` in /tmp, symlink attack vector | `media_keys.rs:152-157` | Use `tempfile` crate or add PID + random suffix; check for symlinks before write |
| S5 | **No HTTPS enforcement** — `koan remote login` accepts `http://` URLs without warning | `commands/remote.rs:6`, `client.rs:31-38` | Warn on non-HTTPS for non-localhost URLs |
| S6 | **Weak random salt fallback** — `/dev/urandom` failure silently produces all-zero salt | `client.rs:331-338` | Use `getrandom` crate (already in dep tree via `uuid`); fail loudly on error |

### LOW / PASS

- No hardcoded secrets in source (PASS)
- All DB queries parameterized via `params![]` (PASS)
- No shell/command execution (PASS)
- Path traversal prevention in organize module via `sanitize_path_component` (PASS)
- TLS enabled via rustls (PASS)
- `unsafe impl Send` on AudioEngine/CallbackData — necessary for CoreAudio FFI, correctly documented (LOW)
- No certificate pinning — acceptable for music player (LOW)
- Recommend running `cargo audit` for known CVEs (INFO)

---

## 2. Code Quality & Dead Code

### Clippy: CLEAN — zero warnings

8 `#[allow(clippy::too_many_arguments)]` suppressions (acceptable for complex internal functions, but `buffer.rs` has 4 — consider builder pattern).

### Dead Code (6 items)

| # | Item | Location | Verdict |
|---|------|----------|---------|
| DC1 | `LyricsState::clear()` | `lyrics.rs:41` | Never called — callers use `self.lrc_lines.clear()` directly |
| DC2 | `CoverArt::centered()` | `cover_art.rs:239` | Zero callers |
| DC3 | `Theme::scrollbar_hover` field | `theme.rs:30` | Defined and defaulted but never read |
| DC4 | `tui::event` module (entire file) | `event.rs:3` | Declared in `mod.rs` but `Event::` never referenced — **entire module unused** |
| DC5 | `VisualizerState::num_bars()` | `visualizer.rs:71` | Zero callers |
| DC6 | 3 unused `HoverZone` variants | `app.rs:84` | `PanelDivider`, `PickerItem`, `ContextMenuItem` — never constructed or matched |

### Production `unwrap()` / `expect()` Concerns

| # | Location | Risk | Fix |
|---|----------|------|-----|
| U1 | `replaygain.rs:239` — `EbuR128::new(...).unwrap()` | Panics if ebur128 fails (invalid sample rate) | Propagate error via `?` — surrounding fn already returns `Result` |
| U2 | `buffer.rs:224` — `File::open(&path).expect("failed to open")` | Panics if file deleted between queue and playback | Return error or handle gracefully |

### No Issues Found

- Zero TODO/FIXME/HACK markers
- Zero commented-out code blocks (all multi-line comments are legitimate documentation)
- No magic numbers (constants are well-named)

---

## 3. Performance

### Priority 1 — High Impact

| # | Issue | Location | Impact | Fix |
|---|-------|----------|--------|-----|
| P1 | **`derive_visible_queue()` rebuilds every frame** — clones all track strings (path, title, artist, album, etc.) for entire playlist every tick | `player/state.rs:701-785` | ~3,500 String clones/frame for 500-track queue | Add `playlist_version` check in `refresh_visible_queue()` to skip rebuild when unchanged |

### Priority 2 — Medium Impact

| # | Issue | Location | Impact | Fix |
|---|-------|----------|--------|-----|
| P2 | **`build_display_lines()` clones strings for album keys** — allocates 2 Strings per entry, called 6 times per frame | `queue.rs:137` | ~6,000 String allocs/frame for 500 tracks | Use `(&str, &str)` borrowed keys instead of cloning |
| P3 | **`build_display_lines()` computed 6 times** — same data, same result | `queue.rs:85,106,191,449,467` | Redundant computation | Compute once per frame, cache alongside `vq_cache` |
| P4 | **String clones in Span construction** — every visible track clones 2-4 strings for ratatui Spans | `queue.rs:294-297,378,435` | ~120 String clones/frame for 30 visible tracks | Ratatui API limitation; consider `Cow<str>` |

### Priority 3 — Low Impact / Polish

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| P5 | `VizFrame.spectrum` uses `Vec<f32>` (heap alloc per frame) | `analyzer.rs:605`, `viz.rs:49` | Convert to `[f32; NUM_BARS]` — eliminates ~120 heap allocs/sec |
| P6 | Seek bar allocates 3 strings per frame via `.repeat()` | `transport.rs:139-142` | Pre-allocated buffer (very low priority) |
| P7 | Decode loop busy-waits with `sleep(500us)` when ring buffer full | `buffer.rs:629` | Consider condvar (acceptable as-is for music player) |
| P8 | `/dev/urandom` opened per API call for salt | `client.rs:331-338` | Use `getrandom` crate |
| P9 | `StreamBuffer` grows unbounded (~50MB for large FLAC) | `streaming.rs:47` | Acceptable for single-track; note if prefetching added |

### Positive Observations

- CoreAudio render callback is textbook real-time safe: zero allocations, zero locks, pure pointer ops
- 3-phase lock discipline in FFT analyzer is well-designed
- Ring buffer (`rtrb`) for audio transport is correct choice
- `SampleBuffer` and ReplayGain scratch buffer are properly reused (no per-packet allocation)
- DB queries use JOINs — no N+1 patterns
- `reqwest` features correctly scoped (no unnecessary defaults)

---

## 4. Dependencies

### HIGH

| # | Issue | Fix |
|---|-------|-----|
| D1 | **`symphonia` uses `features = ["all"]`** — pulls every codec including ADPCM, CAF, MKV that koan never uses | Replace with specific features: `["flac", "mp3", "aac", "vorbis", "alac", "pcm", "isomp4", "ogg", "wav", "aiff"]` — faster builds, smaller binary |

### MEDIUM

| # | Issue | Fix |
|---|-------|-----|
| D2 | **`rusqlite` in both crates** — `koan-music` only uses it for 3 `params![]` calls | Move those to `koan-core` query functions; remove `rusqlite` from `koan-music` |
| D3 | **`rusqlite` uses `bundled-full`** — includes CSV, session, and extensions koan doesn't use | Audit needed features; `bundled` alone provides SQLite + FTS5 |
| D4 | **`core-foundation = "0.9"` direct dep** in `koan-music` — causes duplicate with v0.10 from rustls chain | Check if actually imported directly; may be removable |

### LOW

| # | Issue | Fix |
|---|-------|-----|
| D5 | `walkdir` version mismatch — `"2.5"` in core vs `"2"` in music | Align to `"2.5"` or use workspace deps |
| D6 | `toml` duplicated across crates | Consider workspace dependency declaration |
| D7 | `bitflags` v1 + v2 — transitive from cocoa/souvlaki vs crossterm/ratatui | Unavoidable; will resolve when upstreams update |

### Positive

- `image` crate correctly scoped to `jpeg` + `png` only
- `reqwest` disables defaults, enables only `blocking`, `json`, `query`, `rustls-tls`
- No known CVEs from manual review (recommend `cargo audit` for automated check)

---

## 5. Architecture & File Sizes

### Giant Files — Split Candidates

| File | Lines | Functions | Priority | Recommended Split |
|------|------:|----------:|----------|-------------------|
| `tui/app.rs` | 2,319 | 46 | **P0** | Extract `input_keyboard.rs` (~800), `input_mouse.rs` (~570), `selection.rs` (~130), `queue_ops.rs` (~160) |
| `player/mod.rs` | 1,384 | — | **P1** | Extract undo logic to `player/undo.rs`; move tests to `player/tests.rs`; deduplicate `start_playback`/`start_streaming_playback` (~80 shared lines) |
| `organize.rs` | 1,104 | — | **P2** | Move tests to separate file; deduplicate `plan_moves`/`plan_moves_from_paths` (~60% shared logic) via trait-based metadata provider |
| `db/queries/tracks.rs` | 860 | — | **P2** | Extract `row_to_track` helper (4 copies of 20-column mapping currently duplicated) |
| `audio/buffer.rs` | 671 | — | **P3** | Split into `probe.rs`, `timeline.rs`; keep decode in `buffer.rs` |
| `format/functions.rs` | 1,195 | — | **Skip** | Well-structured match dispatch table — splitting would hurt readability |

### Architecture: Clean

- **No circular dependencies** — strict `koan-music` -> `koan-core` layering
- **Clean concurrency model** — atomics for hot reads, `RwLock` for playlist, ring buffer for audio
- **No god objects** in core — `SharedPlayerState` is well-factored with explicit `bump_version()` pattern
- **`App` struct in app.rs is the one god object** — holds all TUI state and handles all input routing; decomposition plan above addresses this
- **No FFI remnants** — clean break from original SwiftUI architecture; no unused abstractions

### Duplicated Logic

| Location | Issue | Fix |
|----------|-------|-----|
| `player/mod.rs` — `start_playback` vs `start_streaming_playback` | ~80 lines of shared setup (device, sample rate, gapless, engine) | Extract common setup into shared helper |
| `organize.rs` — `plan_moves` vs `plan_moves_from_paths` | ~60% shared logic (sanitization, extension, ancillary, dedup) | Trait-based metadata provider |
| `db/queries/tracks.rs` — `row_to_track` | 4 copies of 20-column row mapping | Extract into single helper function |

---

## 6. Test Coverage & Test Quality

**332 tests total** (318 koan-core + 14 koan-music). All passing.

Strong coverage of pure logic (format functions, replaygain math, streaming buffer I/O, undo/redo, config merging, organize paths). Significant gaps in infrastructure and state-management layers.

### USELESS Tests — AI-Generated Duplicates

| Test | Location | Issue |
|------|----------|-------|
| `test_new_and_is_complete` | `streaming.rs:333` | Duplicates `is_complete_when_done` + `new_buffer_starts_empty`; tests a method (`is_complete()`) that doesn't exist |
| `test_read_all_available` | `streaming.rs:349` | Exact semantic duplicate of `read_all_data_available` (line 188) |
| `test_seek_start` | `streaming.rs:361` | Exact semantic duplicate of `seek_from_start` (line 215) |
| `test_seek_current` | `streaming.rs:373` | Exact semantic duplicate of `seek_from_current` (line 229) |

Comment at line 330 — `// --- Tests using the requested names ---` — dead giveaway of AI generation to match a spec list without checking existing coverage. **4 tests, zero additional coverage. Delete all.**

### WEAK Tests — Low Regression Value

| Test | Location | Issue |
|------|----------|-------|
| `is_seekable_true` | `streaming.rs:295` | Tests a one-liner that unconditionally returns `true` |
| `byte_len_returns_total` | `streaming.rs:287` | Trivial round-trip through mutex; doesn't test concurrent access |
| `analyzer_spawns_and_shuts_down` | `analyzer.rs:632` | Only asserts Vec lengths guaranteed by `Default` impl |
| `test_codec_from_file_type` | `metadata.rs:265` | Tests that a match expression returns string literals |
| `test_defaults` | `config.rs` | Tests that `Default` returns the values written in `impl Default` |
| `test_deserialize_lrclib_*` | `lrclib.rs` | Borderline useful (verify serde rename); keep but improve |

### Coverage Gaps — MISSING High-Value Tests

| Priority | Module | What's Missing |
|----------|--------|----------------|
| **P0** | `audio/buffer.rs` — `PlaybackTimeline` | Zero tests. Binary search arithmetic for position tracking; the seek bar's source of truth. Edge cases: 0 samples, past all boundaries, seek offset math. Pure data structure, fully testable. |
| **P0** | `player/state.rs` — `SharedPlayerState` | Zero tests. All playlist mutation logic: `advance_cursor()`, `peek_next_ready_after()`, `retreat_cursor()`, `derive_visible_queue()` (12 branches mapping cursor/load_state to status), `move_items()`, `item_playback_source()`. Pure in-memory, no I/O dependency. |
| **P1** | `db/queries/favourites.rs` | Zero tests. `toggle_favourite`, `import_remote_favourites` — same in-memory DB pattern as existing query tests. |
| **P1** | `remote/client.rs` — response deserialization | Zero tests. `SubsonicSong`, `SubsonicAlbumFull` JSON round-trip. `auth_params()`, `stream_url()` construction. |
| **P1** | `index/metadata.rs` — `metadata_from_probe_result()` | Not tested. Track number "3/12" split, `OriginalDate` fallback, empty value skipping. |
| **P2** | `lyrics.rs` — `fetch_lyrics()` pipeline | Cache hit → embedded → sidecar → LRCLIB → cache write. Testable with in-memory DB. |
| **P2** | `streaming.rs` — concurrent push + read | Reader blocks mid-stream, writer pushes chunk, reader unblocks. Condvar logic untested. |
| **P2** | `tui/visualizer.rs` — `SpectrumWidget::render()` | Downsample/upsample interpolation, peak-over-bar priority. Testable with `TestBackend`. |
| **P2** | `audio/analyzer.rs` — `AmplitudeScale::parse()` / `FrequencyScale::parse()` | Alias variants (`"a-weight"`, `"a_weight"`, `"logarithmic"`) untested. |
| **P3** | `remote/sync.rs` | Zero tests (hard to unit test without mocking HTTP). |
| **P3** | All TUI modules except visualizer | Zero tests. `app.rs`, `queue.rs`, `library.rs`, `transport.rs` — integration testing with `TestBackend` would be valuable but high effort. |

### Test Distribution Imbalance

| Area | Tests | Lines of Code | Tests/KLOC |
|------|------:|-------------:|----------:|
| `format/` (title formatting) | ~168 | 2,328 | 72 |
| `audio/` (engine, buffer, etc.) | 42 | 3,037 | 14 |
| `player/` (state machine) | 26 | 2,170 | 12 |
| `db/` (queries) | 15 | 1,260 | 12 |
| `config.rs` | 15 | 522 | 29 |
| `organize.rs` | 10 | 1,104 | 9 |
| `tui/` (all UI) | 14 | 4,500+ | 3 |
| `remote/` (sync, client) | 2 | 800+ | 2.5 |

The format engine has excellent coverage at 72 tests/KLOC. The player state machine and audio infrastructure are under-tested relative to their complexity and risk.

---

## Implementation Plan

### Phase 1 — Quick Wins (1-2 days)

1. **S1**: Add `chmod 0o600` to `save_local()` — 5-line fix
2. **S6**: Replace `/dev/urandom` with `getrandom` crate — 3-line fix
3. **S5**: Add HTTPS warning for non-localhost URLs — 10-line fix
4. **S4**: Add PID + random suffix to cover art temp path — 5-line fix
5. **DC1-DC6**: Remove all dead code items — delete unused functions/modules/variants
6. **U1-U2**: Replace panicking `unwrap()`/`expect()` with error propagation
7. **D1**: Replace `symphonia features = ["all"]` with specific codecs

### Phase 2 — Performance (2-3 days)

8. **P1**: Add `playlist_version` check to `refresh_visible_queue()` — biggest single perf win
9. **P2-P3**: Cache `build_display_lines()` output, use borrowed keys
10. **P5**: Convert `VizFrame.spectrum` from `Vec<f32>` to `[f32; NUM_BARS]`

### Phase 3 — Architecture Cleanup (3-5 days)

11. **app.rs decomposition**: Extract `input_keyboard.rs`, `input_mouse.rs`, `selection.rs`, `queue_ops.rs`
12. **player/mod.rs**: Extract undo, deduplicate playback setup
13. **organize.rs**: Deduplicate plan_moves logic
14. **tracks.rs**: Extract `row_to_track` helper

### Phase 4 — Dependency Cleanup (1 day)

15. **D2**: Move rusqlite usage from koan-music to koan-core
16. **D3**: Audit rusqlite feature flags
17. **D4-D6**: Clean up version alignment, workspace deps

### Phase 5 — Test Improvements (2-3 days)

18. **Delete 4 AI-generated duplicate tests** in `streaming.rs:330-411`
19. **Add `PlaybackTimeline` tests** — binary search, seek offset math, edge cases (P0)
20. **Add `SharedPlayerState` tests** — `advance_cursor`, `derive_visible_queue` status branches, `move_items` (P0)
21. **Add `favourites.rs` tests** — toggle round-trip, remote import (P1)
22. **Add Subsonic response deserialization tests** — JSON round-trip for song/album types (P1)
23. **Add `metadata_from_probe_result` tests** — track "3/12" split, OriginalDate fallback (P1)
24. **Improve weak tests** — strengthen `analyzer_spawns_and_shuts_down`, add concurrent streaming test (P2)

---

## Effort Estimate

| Phase | Effort | Priority |
|-------|--------|----------|
| Phase 1 — Quick Wins | 1-2 days | HIGH |
| Phase 2 — Performance | 2-3 days | HIGH |
| Phase 3 — Architecture | 3-5 days | MEDIUM |
| Phase 4 — Dependencies | 1 day | LOW |
| Phase 5 — Tests | 2-3 days | MEDIUM |
| **Total** | **~10-15 days** | |

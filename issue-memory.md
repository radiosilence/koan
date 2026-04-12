# Koan Codebase Audit: Memory Leaks

Following up on the general audit, I've specifically investigated the codebase for memory leaks. While Rust's ownership model prevents most traditional leaks (e.g., forgotten `free()`), I found several instances of **unbounded memory growth** that will cause long-running `koan` instances to slowly leak memory and eventually OOM.

---

## 1. 🚩 TUI `log_messages` Accumulation (Classic Leak)
**Location:** `crates/koan-music/src/tui/app.rs`

In the TUI's main render loop, logs from background threads are continuously drained from the shared `log_buffer` and appended to `self.log_messages`:
```rust
// Drain log buffer.
if let Ok(mut logs) = self.log_buffer.lock() {
    self.log_messages.extend(logs.drain(..));
}
```
**The Issue:** There is absolutely no truncation or length limit applied to `self.log_messages`. Every single log emitted by background downloads, radio queries, or scans is kept in heap memory forever.
**Impact:** A user running the TUI for days/weeks will leak gigabytes of `String` allocations, eventually causing an Out-Of-Memory (OOM) crash.

---

## 2. 🚩 `Playlist::items` Unbounded Growth (Radio Mode)
**Location:** `crates/koan-core/src/player/state.rs` and `crates/koan-music/src/tui/app.rs`

**The Issue:** The `Playlist` struct stores the queue as a flat `Vec<PlaylistItem>`. When Radio Mode is active, the `trigger_radio_pick()` background thread continuously generates new tracks and appends them to the playlist to keep playback going.
**The Problem:** The application *never* truncates the history of already-played tracks from the front of the queue.
**Impact:** If a user leaves `koan` playing in radio mode continuously, the playlist vector will grow to tens of thousands of items, permanently holding onto `String`s (titles, artists, albums) and `PathBuf`s for every song ever played in that session.

---

## 3. 🚩 `PlaybackTimeline::boundaries` Accumulation
**Location:** `crates/koan-core/src/audio/buffer.rs`

**The Issue:** The gapless playback engine tracks song transitions using `boundaries: parking_lot::RwLock<Vec<TrackBoundary>>`. While `timeline.reset()` clears this vector, it is *only* called when playback is explicitly stopped or the queue is fully cleared. 
**The Problem:** If the user is listening sequentially or in radio mode, new boundaries are pushed on every track transition. The old boundaries are never culled.
**Impact:** A slow memory leak during uninterrupted playback sessions. While smaller in footprint than the strings above, it is still structurally unbounded.

---

### Recommended Fixes
1. **Logs:** Apply a hard cap to `App::log_messages` (e.g., `if self.log_messages.len() > 1000 { self.log_messages.drain(0..500); }`).
2. **Playlist Culling:** Introduce an upper bound for the playlist length, or automatically cull `PlaylistItem`s that are hundreds of indices behind the active `cursor` when in Radio Mode.
3. **Boundary Pruning:** Periodically prune `TrackBoundary` elements from the timeline that correspond to `sample_offset`s far behind the current `samples_played`.
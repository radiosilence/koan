# Plan 08: Wire ReplayGain into Playback

## Status quo

**What exists:**
- `audio/replaygain.rs` — full implementation: `read_tags()`, `apply_gain()`, `select_gain()`, `scan_track()`, `scan_album()`, `write_tags()`. All well-tested.
- `config.rs` — `PlaybackConfig.replaygain: ReplayGainMode` (Off/Track/Album). Defaults to `Album`.
- `apply_gain()` accepts `pre_amp_db` parameter — ready for future DSP preamp config.

**What's missing:**
- Nobody ever calls `read_tags()` or `apply_gain()` during playback. The decode loop in `buffer.rs` pushes raw samples straight to the ring buffer with zero processing.
- No `pre_amp_db` config field yet (fine — pass `0.0` for now, add config later with DSP work).
- RG values are NOT stored in the DB schema. No columns for track_gain/album_gain/etc. Tags are read at playback time via lofty, which is the correct approach (same as foobar2000).

## Where gain gets applied

`buffer.rs`, `decode_single()` — lines 436-437:
```rust
sbuf.copy_interleaved_ref(decoded);
let samples = sbuf.samples();
```

The gain needs to be applied to a mutable copy of `samples` right here, before the ring buffer push loop (lines 440-468). `sbuf.samples()` returns `&[f32]` (immutable), so we need a mutable scratch buffer.

## Architecture decision

Read RG tags once per track at the start of `decode_single()`, not from the DB. Reasons:
- Tags are the source of truth (user may have just rescanned)
- `lofty::read_from_path` is fast (just reads tag headers, no decode)
- The file is already being opened for decode anyway
- Avoids threading config/DB handles through the decode path

The `ReplayGainMode` config value needs to reach the decode thread. Two options:
1. **Pass it through `start_decode()`** — cleanest, no global state
2. Load config inside decode thread — wasteful, config could change mid-track

Option 1. Thread the mode through.

## Edge cases

- **No RG tags**: `select_gain()` returns `None` -> skip `apply_gain()`, play at unity gain. No fallback gain.
- **Album mode, no album tags**: `select_gain()` already falls back to track gain.
- **Clipping prevention**: `apply_gain()` already limits gain using peak values when available.
- **Mode = Off**: `select_gain()` returns `None` -> no processing. Zero overhead.
- **Gapless transitions**: Each track in `decode_queue_loop` calls `decode_single()` independently, so each gets its own RG tags. Correct behavior.
- **Pre-amp**: Hardcode `0.0` now. Add `pre_amp_db: f64` to `PlaybackConfig` later (plan 02 DSP work).

## Implementation steps

1. **Add `pre_amp_db` to `PlaybackConfig`** — `f64`, default `0.0`, serde. Small but avoids a hardcode.

2. **Thread config into decode path** — Add `replaygain_mode: ReplayGainMode` and `pre_amp_db: f64` params to:
   - `start_decode()`
   - `decode_queue_loop()`
   - `decode_single()`

3. **Read RG tags in `decode_single()`** — At the top, after opening the file, call `replaygain::read_tags(path)`. Log on error, don't fail playback. Call `replaygain::select_gain(&info, mode)` to get the active `(gain_db, peak)` or `None`.

4. **Apply gain in the decode loop** — After `sbuf.copy_interleaved_ref(decoded)`, if gain is active:
   - Copy `sbuf.samples()` into a reusable `Vec<f32>` scratch buffer
   - Call `replaygain::apply_gain(&mut scratch, gain_db, peak, pre_amp_db)`
   - Push from scratch buffer instead of `sbuf.samples()`
   - If no gain, push directly from `sbuf.samples()` as before (zero overhead for Off mode)

5. **Pass config from Player** — In `Player::start_playback()`, load config (`Config::load()`) and pass `replaygain` + `pre_amp_db` to `buffer::start_decode()`. Config is loaded once per playback start, not per track — consistent within a session. Alternative: store config on the `Player` struct, reload on a `ReloadConfig` command.

6. **Tests** — Unit tests for the wiring aren't really needed (the replaygain module is already well-tested). Integration-level: verify that `decode_single` with `ReplayGainMode::Off` doesn't alter samples vs. the current behavior. Quick sanity test.

7. **Config docs** — Update the generated config template in `commands/config.rs` to document `pre_amp_db`.

## Rough LOC estimate

~40-60 lines changed across buffer.rs + player/mod.rs + config.rs. Small diff.

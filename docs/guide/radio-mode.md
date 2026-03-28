# Radio Mode

Radio mode turns koan into an infinite jukebox. When enabled, koan automatically queues similar tracks as you listen -- you never run out of music.

## Quick start

Press `R` in the TUI to toggle radio mode. That's it. koan starts adding tracks to your queue based on what's playing.

## How it works

Radio mode uses a multi-signal scoring system to find tracks similar to what you're currently listening to:

1. **ListenBrainz similar artists** -- ML-based artist similarity fetched live from the ListenBrainz API. Results are cached locally so subsequent lookups are instant.

2. **MusicBrainz relationships** -- fetches artist relationships (collaborators, band members, associated acts) from the MusicBrainz API via MBID lookups. Also cached locally. Rate-limited per MusicBrainz terms of service.

3. **Subsonic similarity** (`use_subsonic`) -- if you have a remote server configured, koan queries `getSimilarSongs2` for server-side recommendations and caches the results.

4. **Genre and era matching** -- tracks sharing genres and release era with the current seed tracks get a relevance boost.

5. **Same-artist tracks** -- other tracks by the same artist or related artists score higher. Local library fallback when external APIs aren't available.

6. **Acoustic similarity** -- if you've run `koan scan --analyze`, radio mode factors in acoustic features (spectral centroid, energy, tempo) via vector KNN. Control the weight with `[discovery] acoustic_weight`.

7. **Random library tracks** -- final fallback when other signals don't produce enough candidates.

The scoring system blends these signals and avoids repeating recently-played tracks (controlled by `history_window`).

## Configuration

```toml
[radio]
lookahead = 5                 # tracks to keep queued ahead (default: 5)
batch_size = 5                # tracks added per refill (default: 5)
use_subsonic = true           # use Subsonic similarity when available (default: true)
history_window = 200          # don't repeat last N tracks (default: 200)
seed_window = 5               # recent tracks used as seed for similarity (default: 5)
discovery_weight = 0.3        # 0.0 = familiar only, 1.0 = maximize discovery (default: 0.3)
```

### Tuning discovery

`discovery_weight` is the most impactful setting:

| Value | Behavior |
|-------|----------|
| `0.0` | Stick to what you know -- heavily favours familiar artists and genres |
| `0.3` | **(default)** Balanced mix of familiar and new discoveries |
| `0.7` | Adventurous -- actively seeks out less-played tracks and artists |
| `1.0` | Maximum exploration -- prioritizes tracks you've never heard |

### Seed window

`seed_window` controls how many recent tracks inform the "similar to what?" query. With the default of 5, radio mode looks at the last 5 tracks to determine the musical direction. A smaller window (1-2) makes the radio more reactive to the single current track; a larger window (10+) gives a broader, more averaged vibe.

### Lookahead and batch size

`lookahead` is how many tracks radio mode tries to keep queued ahead of the current position. When the queue runs below this threshold, it adds `batch_size` more tracks. The default of 5 for both means you always have ~5 tracks ahead, refilled in batches of 5.

## Acoustic analysis

For the best radio experience, run acoustic analysis on your library:

```bash
koan scan --analyze
```

This computes acoustic features (spectral centroid, energy, tempo estimates) for each track. With acoustic analysis data available, radio mode can find tracks that genuinely *sound* similar, not just tracks that share metadata.

Control how much weight acoustic similarity gets:

```toml
[discovery]
analysis_on_scan = false      # run automatically during every scan (default: false)
acoustic_weight = 0.5         # 0.0 = metadata only, 1.0 = acoustics only (default: 0.5)
```

Setting `analysis_on_scan = true` runs acoustic analysis on every `koan scan`, keeping features up to date as you add music. This makes scans slower, so it's off by default -- run `koan scan --analyze` manually when you want to update.

## Tips

- **Start with a track you like.** Radio mode uses whatever's playing as its seed. Queue up a track that sets the vibe you want, then press `R`.
- **Queue some variety first.** If you queue tracks from different genres before enabling radio, the seed window will pick up on the mix and produce more varied results.
- **Still edit the queue.** Radio mode only adds tracks -- you can still remove tracks you don't want (`e` to edit, `d` to delete). Radio will refill around your changes.
- **Works offline.** Without a Subsonic server, radio falls back to local genre/artist matching and acoustic similarity. Less accurate but still functional.

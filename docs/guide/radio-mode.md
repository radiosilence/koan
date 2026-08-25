# Radio Mode

Radio mode turns koan into an infinite jukebox. When enabled, koan keeps the queue topped up from your own library as you listen -- you never run out of music.

## Quick start

Press `R` in the TUI to toggle radio mode. That's it. koan starts adding tracks to your queue based on what's playing.

## How it works

Radio mode scores every candidate track on several signals and picks from the top
with a weighted random draw, so the same seed does not produce the same queue twice.

Every signal it uses is a database read:

1. **Genre and era matching** -- tracks sharing genres with the seed window score
   higher, and tracks from within five years of the seed's average year higher again.

2. **Same artist** -- other tracks by the artists in the seed window.

3. **Acoustic similarity** -- if you have run `koan scan --analyze`, koan takes the
   centroid of the seed tracks' feature vectors and finds its nearest neighbours.
   This is the only signal that hears the music rather than reading about it, and
   it is the one worth turning on.

4. **Random** -- a handful of tracks from the rest of the library, scored low enough
   that they only win when nothing else has anything to say. On an unanalysed library
   with sparse genre tags this is most of what you get.

On top of those, tracks matching on more than one signal are boosted, and tracks you
have not played recently -- or ever -- are boosted by `discovery_weight`. Anything in
the last `history_window` plays, or already in the queue, is excluded outright.

### What it does not use

koan can query **ListenBrainz** and **MusicBrainz** for similar artists, and
**Subsonic `getSimilarSongs2`** for server-side recommendations. None of them run
when radio picks a track.

They are synchronous HTTP calls, and the two metadata services rate-limit themselves
to one request per second *per seed artist*, so a queue about to run dry would get a
better-chosen track some seconds after the music had already stopped. The code is
still there, behind an `allow_network` flag that the auto-queue loop turns off, and
it is waiting on a background pass that can fill the similar-artist cache while there
is time for it rather than in front of a pick that is needed now.

Until then: radio is local metadata, acoustic vectors if you have them, and a random
tail. It is not asking anyone what sounds like what.

Two consequences worth knowing, since they follow from the same gap:

- The `similar_artists` cache is only ever written by those network signals, so it
  stays empty. `similarArtists` over GraphQL and FFI, and `getSimilarSongs2` on
  koan's own Subsonic API, return nothing.
- Radio works exactly the same offline as online. That is not the graceful
  degradation it looks like -- there is no online path to degrade from.

## Configuration

```toml
[radio]
lookahead = 5                 # tracks to keep queued ahead (default: 5)
batch_size = 5                # tracks added per refill (default: 5)
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

This computes acoustic features (tempo, timbre, chroma, and spectral features — a 23-dimensional vector) for each track using bliss-audio. It is the difference between radio finding tracks that genuinely *sound* similar and radio shuffling things that share a genre string.

```toml
[library]
analyze_on_scan = false       # run automatically during every scan (default: false)
```

Setting `analyze_on_scan = true` runs acoustic analysis on every `koan scan`, keeping features up to date as you add music. This makes scans slower, so it's off by default -- run `koan scan --analyze` manually when you want to update.

## Tips

- **Start with a track you like.** Radio mode uses whatever's playing as its seed. Queue up a track that sets the vibe you want, then press `R`.
- **Queue some variety first.** If you queue tracks from different genres before enabling radio, the seed window will pick up on the mix and produce more varied results.
- **Still edit the queue.** Radio mode only adds tracks -- you can still remove tracks you don't want (`e` to edit, `d` to delete). Radio will refill around your changes.
- **Run the analysis.** `koan scan --analyze` is the single biggest thing you can do for radio quality. Without it, radio has only genre tags, artist names and chance to work with -- and genre tags on a ripped library are often blank or wrong.

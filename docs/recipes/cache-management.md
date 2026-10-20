# Cache management

When you play remote tracks, kōan downloads them to a local cache so later plays read from disk.

## Check cache status

```bash
koan cache status
```

Shows the total size, number of cached tracks, and the cache directory path.

## Cache location

Default: `~/.config/koan/cache/`

Override with:
```toml
[remote]
cache_dir = "/path/to/custom/cache"
```

Or: `KOAN_REMOTE__CACHE_DIR=/path/to/custom/cache`

## How far ahead to download

```toml
[remote]
download_ahead = 3
```

The number of tracks past the playing one that are fetched before they are reached. Unset, the whole queue is fetched, which is what to want before going somewhere without a connection; a number keeps a long queue from downloading in full when it will be listened to with a connection anyway. `0` fetches only the track being played: nothing plays gaplessly, since the decoder has nothing to read ahead into, and each track waits for its download before it starts. `1` is the least that keeps gapless playback. Settings → Downloads → **Download ahead** offers 0 to 10 and the whole queue.

## Size limit

```toml
[remote]
cache_limit = "50GB"
```

In Settings this is **Keep downloads**: up to a limit (in GB), without limit, or only what plays next, which writes `cache_limit = "0"`.

With a limit set, the queue is fetched only as far ahead as the limit allows, and no further than `download_ahead`: the playing track, and the next unless `download_ahead` is 0 (gapless playback reads ahead), then each following track while the cache stays under the limit. The rest of the queue waits and is fetched as the cursor reaches it. Without this a long queue downloaded in full and then could not be evicted.

Eviction takes, in order:
1. Downloads fetched for playback, least recently used first. Tracks already played from the queue are among them, as is anything past the window above.
2. Downloads you asked for with **Download to Cache**, least recently used first. These are pinned, not permanent: they go only once nothing fetched for playback is left to remove.

It works a file at a time, so a part-played album loses only the tracks already played. It never takes a track from an album with a favourited track, nor a track in the window ahead of the cursor. Eviction runs at startup, at most once a minute as downloads land, and as soon as the limit changes in Settings. Size comes from the database, not a filesystem walk.

If no `cache_limit` is set, nothing is evicted and the cache grows without bound.

### No cache

`cache_limit = "0"` keeps no cache. The tracks `download_ahead` reaches are fetched as usual, and every other track fetched to play is deleted once the cursor has passed it. Downloads you asked for stay until you remove them, as do tracks from albums with a favourite, so this is the setting for a phone that should hold what was chosen for offline listening and nothing else.

## Manual eviction

```bash
koan cache evict          # run LRU eviction based on cache_limit
```

## Clear everything

```bash
koan cache clear          # delete all cached downloads
```

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

## Size limit

```toml
[remote]
cache_limit = "50GB"
```

With a limit set, the queue is fetched only as far ahead as the limit allows: the playing track and the next always (gapless playback reads ahead), then each following track while the cache stays under the limit. The rest of the queue waits and is fetched as the cursor reaches it. Without this a long queue downloaded in full and then could not be evicted.

Eviction takes, in order:
1. Downloads fetched for playback, least recently used first. Tracks already played from the queue are among them, as is anything past the window above.
2. Downloads you asked for with **Download to Cache**, least recently used first. These are pinned, not permanent: they go only once nothing fetched for playback is left to remove.

It works a file at a time, so a part-played album loses only the tracks already played. It never takes a track from an album with a favourited track, nor a track in the window ahead of the cursor. Eviction runs at startup, at most once a minute as downloads land, and as soon as the limit changes in Settings. Size comes from the database, not a filesystem walk.

If no `cache_limit` is set, the whole queue is fetched and the cache grows without bound.

## Manual eviction

```bash
koan cache evict          # run LRU eviction based on cache_limit
```

## Clear everything

```bash
koan cache clear          # delete all cached downloads
```

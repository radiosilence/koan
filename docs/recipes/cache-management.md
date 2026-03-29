# Cache Management

When you play remote tracks, koan downloads them to a local cache so subsequent plays are instant. This guide covers monitoring and managing that cache.

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

## Automatic LRU eviction

Set a size limit and koan evicts the least-recently-played tracks on startup:

```toml
[remote]
cache_limit = "50GB"
```

Eviction rules:
- Evicts whole albums (not individual tracks), oldest last-played first
- **Favourited tracks are never evicted** -- starring a track protects it from cache cleanup
- Eviction runs automatically when koan starts, not during playback
- Size is calculated from the database (fast), not by scanning the filesystem

If no `cache_limit` is set, the cache grows without bound.

## Manual eviction

```bash
koan cache evict          # run LRU eviction based on cache_limit
```

Useful if you want to trigger eviction without restarting koan.

## Clear everything

```bash
koan cache clear          # delete all cached downloads
```

This removes all cached files. Remote tracks will need to re-download on next play.

## How caching interacts with playback

- Remote tracks start playing after **256KB** is buffered (progressive download)
- Once fully downloaded, the file is cached and metadata/cover art are re-read
- The seek bar dims the not-yet-downloaded portion during progressive playback

## Drive unplugged scenario

If your local music drive is disconnected, tracks that also exist on your remote server are automatically demoted to remote-only (streaming with cache). When the drive comes back, `koan scan` re-merges them as local files. The cache for those tracks is then redundant and will eventually be evicted by LRU if you have a cache limit set.

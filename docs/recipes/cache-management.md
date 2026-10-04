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

## Automatic LRU eviction

Set a size limit and kōan evicts the least-recently-played tracks on startup:

```toml
[remote]
cache_limit = "50GB"
```

Eviction rules:
- Evicts whole albums (not individual tracks), oldest last-played first
- Favourited tracks are never evicted
- Eviction runs when kōan starts and again as each download lands
- Size is calculated from the database (fast), not by scanning the filesystem

If no `cache_limit` is set, the cache grows without bound.

## Manual eviction

```bash
koan cache evict          # run LRU eviction based on cache_limit
```

## Clear everything

```bash
koan cache clear          # delete all cached downloads
```

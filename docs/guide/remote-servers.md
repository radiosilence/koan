# Remote Servers

koan integrates with [Navidrome](https://www.navidrome.org/), Subsonic, and any server with a Subsonic-compatible API. Remote tracks merge seamlessly with your local library into a single unified collection.

## Setup

```bash
koan remote login https://music.example.com admin
```

This prompts for your password and saves credentials to `config.local.toml` (gitignored). Then sync your library:

```bash
koan remote sync
```

The first sync fetches your entire remote library. This can take a while for large collections (tens of thousands of tracks), but progress is displayed throughout.

## Incremental sync

After the first full sync, subsequent runs are **incremental** -- only albums added since the last sync are fetched:

```bash
koan remote sync          # incremental (fast)
koan remote sync --full   # force complete re-sync
```

Run `koan remote sync` periodically (or after adding music to your server) to pull new tracks.

## How merging works

When you have both local files and a remote server, koan deduplicates tracks using a 3-strategy match:

1. **File path** -- exact local path match
2. **Remote ID** -- Subsonic server ID
3. **Content match** -- artist + album + title + track number

If the same track exists in both sources, it becomes a single database entry. Playback priority:

1. **Local file** -- always preferred (bit-perfect from disk)
2. **Cached download** -- previously downloaded remote tracks
3. **Remote stream** -- on-demand progressive download

### Drive unplugged?

If a local drive is disconnected, tracks with remote backing are demoted to remote-only (streaming fallback) instead of deleted. When the drive comes back, the next `koan scan` re-merges them automatically.

## Streaming playback

Remote tracks start playing after just **256KB** is buffered instead of waiting for the full download. In the TUI:

- The seek bar dims the not-yet-downloaded portion
- Seeking past the downloaded boundary is prevented
- Duration always shows the full track length
- When the download finishes, full metadata and cover art are re-read

Downloads happen in the background with configurable parallelism:

```toml
[remote]
download_workers = 5    # parallel download threads (default: 5)
```

## Transcoding

By default, koan requests the original quality from the server. If bandwidth is a concern:

```toml
[remote]
transcode_quality = "original"   # original | opus-128 | mp3-320
```

| Quality | Description |
|---------|-------------|
| `original` | **(default)** Bit-perfect, whatever the server has |
| `opus-128` | Opus at 128kbps -- transparent quality, ~1/10th the bandwidth of FLAC |
| `mp3-320` | MP3 at 320kbps -- maximum quality lossy, widest compatibility |

Note: transcoding depends on the server supporting it. Navidrome and most Subsonic servers handle this natively.

## Cache management

Downloaded remote tracks are cached locally so subsequent plays are instant. See [Cache Management](../recipes/cache-management.md) for size limits, eviction, and cleanup.

```toml
[remote]
cache_limit = "50GB"           # max cache size, LRU eviction on startup (default: unlimited)
cache_dir = "/custom/path"     # explicit cache dir (default: ~/.config/koan/cache)
```

## Favourite sync

Favourites sync bidirectionally with your remote server:

- Star a track in koan (`f`) -> stars it on the server
- Star a track on the server (via Navidrome web UI, DSub, etc.) -> next `koan remote sync` picks it up

## Configuration reference

```toml
# config.local.toml
[remote]
enabled = true
url = "https://music.example.com"
username = "admin"
# password saved by `koan remote login`

# config.toml or config.local.toml
[remote]
transcode_quality = "original"   # original | opus-128 | mp3-320
download_workers = 5             # parallel download threads
cache_limit = "50GB"             # max cache size (LRU eviction)
cache_dir = "/custom/path"       # cache directory
```

## Checking status

```bash
koan remote status
```

Shows the configured server URL, username, last sync time, and track counts.

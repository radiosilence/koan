# Remote servers

kōan integrates with [Navidrome](https://www.navidrome.org/), Subsonic, and any server with a Subsonic-compatible API. Remote tracks merge with your local library into one collection.

## Setup

```bash
koan remote login https://music.example.com admin
```

This prompts for your password and saves credentials to `config.local.toml` (gitignored). Then sync your library:

```bash
koan remote sync
```

A full sync lists songs a page at a time, about a hundred requests for fifty
thousand tracks; between an iPhone and a kōan server that is about a second
per 10,000 tracks. A server that does
not answer an empty `search3` query is walked an album at a time instead, which
for a library that size takes minutes.

## Staying in sync

Every sync walks the whole library, which is what lets it notice a track the server deleted or gave a new id. The apps sync on their own, and only when the server's library has changed:

- A **koan server** tells every linked app when its library or a playlist changes, and queues the sync for an app that is away.
- **Navidrome and other Subsonic servers** are checked on a timer (`auto_sync_interval_mins`). Each check asks `getIndexes` for the library's `lastModified` and syncs only if it moved since the last sync.

```bash
koan remote sync          # sync now
```

## How merging works

When you have both local files and a remote server, kōan deduplicates tracks by trying, in order:

1. **File path** -- exact local path match
2. **Remote ID** -- Subsonic server ID
3. **Content match** -- artist + album + disc + track number + title, then the same without the artist
4. **MusicBrainz ids** -- recording + release

If the same track exists in both sources, it becomes a single database entry. Playback priority:

1. **Local file** -- always preferred (bit-perfect from disk)
2. **Cached download** -- previously downloaded remote tracks
3. **Remote stream** -- on-demand progressive download

### Drive unplugged?

If a local drive is disconnected, tracks with remote backing are demoted to remote-only (streaming fallback) instead of deleted. When the drive comes back, the next `koan scan` re-merges them automatically.

## Streaming playback

Remote tracks start playing after **256KB** is buffered instead of waiting for the full download.

- Both front ends draw the fetched extent on the seek bar, weaker than the played one. It is a fraction of bytes on an axis of time, so it is right for lossless and CBR and drifts with the bitrate on VBR -- read it as whether playback is about to run out of track, not as a position
- The TUI also refuses a seek past the downloaded boundary
- Duration always shows the full track length
- When the download finishes, full metadata and cover art are re-read

Downloads happen in the background with configurable parallelism:

```toml
[remote]
download_workers = 5    # parallel download threads (default: 5)
```

## Cache management

Played tracks are kept, so they play again from disk. See [Cache management](../recipes/cache-management.md) for size limits and eviction.

```toml
[remote]
cache_limit = "50GB"           # max cache size; bounds queue prefetch too (default: unlimited)
cache_dir = "/custom/path"     # explicit cache dir (default: ~/.config/koan/cache)
```

## Favourites and playlists

Favourites and playlists sync both ways. A change made in kōan is sent to the server at once; one made in another client arrives with the next sync.

## Play history

Each device records every track it starts, and scrobbles to the server the ones heard long enough to count as plays: half the track or four minutes, and nothing under thirty seconds, as Last.fm counts them. A scrobble that cannot be sent waits on the device and goes, dated to when the track started, once the server answers again.

Signed in to a kōan server, history is shared. The server keeps the account's plays, and each device's History shows the plays the account's other devices made alongside its own, arriving as they are scrobbled. A device's own skips stay on that device. Forgetting a play, or clearing history, forgets it on the server and on every device on the account, including plays recorded before history was shared. A Navidrome server keeps plays too, but has nothing to share them with, so each device shows only its own.

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
download_workers = 5             # parallel download threads
cache_limit = "50GB"             # max cache size (LRU eviction)
cache_dir = "/custom/path"       # cache directory
```

## Checking status

```bash
koan remote status
```

Shows the configured server URL, username, last sync time, and track counts.

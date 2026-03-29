# CLI Reference

koan is a single binary with subcommands. Running `koan` with no subcommand launches the TUI player.

## `koan play`

Play audio files or open the TUI player. Running `koan` with no subcommand is equivalent to `koan play`.

```bash
koan                                    # TUI + GraphQL API on :4000
koan play                               # same as above
koan play ~/Music/album/                # play a directory (recursive)
koan play ~/Music/*.flac                # play specific files
koan play --album 5                     # play album by ID (use tab completion)
koan play --artist 3                    # play artist by ID
koan play --library                     # TUI in library browse mode
koan play --clear                       # clear persisted queue
koan --no-api                           # TUI only (no GraphQL server)
```

### Server flags

These are root-level flags (not under `play`).

```bash
koan --headless                   # GraphQL API on 127.0.0.1:4000, no TUI
koan --headless --playground      # with GraphiQL web IDE
koan --headless --subsonic 4040   # + Subsonic REST on port 4040
koan --port 8080                  # custom GraphQL port
koan --bind 0.0.0.0              # listen on all interfaces (no auth)
koan -d                           # background daemon
koan -d --subsonic 4040           # daemon with Subsonic
```

### Remote TUI

```bash
koan play --server http://host:4000          # TUI connected to remote koan
koan play --server http://host:4000 --jukebox  # remote control only
```

### MCP server

```bash
koan mcp                        # MCP server on stdio (Claude Desktop)
```

See [MCP Integration](../guide/mcp-integration.md) for setup instructions.

---

## `koan init`

Create the config directory with sensible defaults.

```bash
koan init
```

Safe to re-run -- merges new default fields without overwriting your customizations.

See [Configuration](configuration.md) for details on what gets created.

---

## `koan scan`

Scan configured library folders and index metadata.

```bash
koan scan                         # standard metadata scan
koan scan --analyze               # scan + acoustic analysis in one pass
```

Scanning runs in parallel using rayon. Subsequent scans are incremental -- only new or modified files are re-indexed (based on mtime + size from the scan cache).

The `--analyze` flag computes acoustic features for radio mode similarity scoring. This is slower than a plain scan.

---

## `koan search`

Full-text search across your library (CLI output).

```bash
koan search "radiohead"
koan search "kind of blue"
```

Uses SQLite FTS5 for fast prefix and stemming search. Results display as a tree: artist -> album -> track.

---

## `koan library`

Show library statistics.

```bash
koan library
```

---

## `koan remote`

Manage Subsonic/Navidrome remote servers.

```bash
koan remote login URL user        # authenticate (prompts for password)
koan remote sync                  # incremental sync (new albums since last sync)
koan remote sync --full           # full re-sync of entire library
koan remote status                # show remote server info
```

See [Remote Servers](../guide/remote-servers.md) for the full guide.

---

## `koan config`

Show the resolved configuration from all layers.

```bash
koan config
```

Displays defaults, config.toml values, config.local.toml overrides, and active `KOAN_*` environment variables with the final merged result.

---

## `koan devices`

List available audio output devices.

```bash
koan devices
```

Shows device names as recognized by CoreAudio (macOS) or ALSA/cpal (Linux). Use these names for the `[playback] output_device` config field or the `Shift+D` device selector in the TUI.

---

## `koan cache`

Manage the download cache for remote tracks.

```bash
koan cache status                 # show cache size and track count
koan cache clear                  # clear all cached downloads (--yes/-y to skip confirmation)
koan cache evict                  # run LRU eviction based on cache_limit
```

See [Cache Management](../recipes/cache-management.md) for details.

---

## `koan probe`

Show format and codec info for a file.

```bash
koan probe track.flac
```

Displays codec, sample rate, bit depth, channels, duration, and tag summary.

---

## Shell completions

Dynamic completions that know your library -- artist/album IDs tab-complete from the database.

```bash
# zsh (add to .zshrc)
source <(COMPLETE=zsh koan)

# bash
source <(COMPLETE=bash koan)

# fish
COMPLETE=fish koan | source
```

Then `koan play --album <TAB>` shows your actual albums with artist names.

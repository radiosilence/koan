# CLI Reference

koan is a single binary with subcommands. Running `koan` with no subcommand launches the TUI player.

## Play (default)

`koan` is the play command. All top-level flags are play flags.

```bash
koan                              # TUI + GraphQL API on :4000
koan ~/Music/album/               # play a directory (recursive)
koan ~/Music/*.flac               # play specific files
koan --album 5                    # play album by ID (use tab completion)
koan --artist 3                   # play artist by ID
koan --library                    # TUI in library browse mode
koan --clear                      # clear persisted queue
koan --no-api                     # TUI only (no GraphQL server)
```

### Server flags

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
koan --server http://host:4000          # TUI connected to remote koan
koan --server http://host:4000 --jukebox  # remote control only
```

### MCP server

```bash
koan mcp                          # MCP server on stdio (Claude Desktop)
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
koan scan /path/to/music          # scan a specific directory
koan scan --force                 # force re-scan of all files (ignore cache)
koan scan --analyze               # scan + acoustic analysis in one pass
```

Scanning runs in parallel using rayon. Subsequent scans are incremental -- only new or modified files are re-indexed (based on mtime + size from the scan cache). Use `--force` to bypass the cache and re-index everything.

The `--analyze` flag computes acoustic features for radio mode similarity scoring. This is slower than a plain scan.

---

## `koan search`

Full-text search across your library (CLI output).

```bash
koan search "radiohead"
koan search "kind of blue"
```

Uses SQLite FTS5 for fast, typo-tolerant search. Results display as a tree: artist -> album -> track.

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
koan cache clear                  # clear all cached downloads
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

## `koan analyze`

Run acoustic analysis on the library for similarity features (used by radio mode).

```bash
koan analyze
```

This computes spectral centroid, energy, and tempo estimates for each track. Equivalent to running `koan scan --analyze` but without re-scanning metadata -- useful when your library is already indexed and you just want to add acoustic data.

---

## `koan completions`

Generate static shell completion scripts for the CLI structure.

```bash
koan completions zsh              # zsh completions
koan completions bash             # bash completions
koan completions fish             # fish completions
```

Add to your shell config:

```bash
# zsh (add to .zshrc)
source <(koan completions zsh)

# bash
source <(koan completions bash)

# fish
koan completions fish | source
```

koan also supports dynamic completions that know your library -- artist/album IDs tab-complete from the database. These are activated automatically via the `COMPLETE` env var when using a compatible shell.

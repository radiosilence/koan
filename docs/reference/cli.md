# CLI

kōan is a single binary with subcommands. Running `koan` with no subcommand launches the TUI player.

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
koan --headless                   # serve on 127.0.0.1:4000, no TUI
koan --headless --playground      # with GraphiQL at /graphql
koan --headless --subsonic 4040   # Subsonic also on a port of its own
koan --port 8080                  # custom GraphQL port
koan --bind 0.0.0.0              # listen on all interfaces (auth enabled by default)
koan -d                           # background daemon
koan -d --subsonic 4040           # daemon with Subsonic
```

### Remote TUI

```bash
koan play --server http://host:4000          # TUI connected to remote koan
koan play --server http://host:4000 --jukebox  # remote control only
```

Client mode pulls audio from the server's `/rest/stream`, which is guarded by the
server's `[subsonic]` credentials rather than the JWT the GraphQL side uses. Set the
same username and secret in *this* machine's config (`koan subsonic setup`, then copy
the secret from the server) or the queue plays nothing. `--jukebox` needs no
credentials — the server does the playing.

### MCP server

```bash
koan mcp                        # MCP server on stdio (Claude Desktop)
```

See [MCP Integration](../guide/mcp-integration.md) for setup instructions.

---

## `koan config init`

Create the config directory with a commented template.

```bash
koan config init
```

Writes `config.toml` with every default commented out. Re-running adds new defaults without touching your changes.

See [Configuration](configuration.md) for details on what gets created.

---

## `koan scan`

Scan configured library folders and index metadata.

```bash
koan scan                         # standard metadata scan
```

Only files whose modification time or size changed are re-read; `--force` re-reads everything. See [Getting started](../getting-started.md#removed-files) for `--force-remove`.

---

## `koan search`

Full-text search across your library (CLI output).

```bash
koan search "radiohead"
koan search "kind of blue"
```

Uses SQLite FTS5 with prefix matching. Results display as a tree: artist -> album -> track.

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
koan remote sync                  # sync the library, favourites and playlists
koan remote status                # show remote server info
```

See [Remote Servers](../guide/remote-servers.md) for the full guide.

---

## `koan subsonic`

Manage kōan's own Subsonic-compatible REST API at `/rest/*`.

```bash
koan subsonic setup               # generate a secret and enable the API
koan subsonic setup --username me # pick the username (default: koan)
koan subsonic status              # show whether it is enabled and configured
koan subsonic disable             # stop serving /rest/* and delete the secret
```

The secret is separate from your `[remote]` (Navidrome) password and is printed once. See [Configuration](configuration.md#subsonic) for why.

---

## `koan config`

Show the resolved configuration from all layers.

```bash
koan config
```

Prints which config files were read, the names of any active `KOAN_*` environment variables, and the merged result, with secrets masked.

---

## `koan dsp`

Equalisation and convolution profiles, per output device. See
[Equalisation and convolution](../guide/dsp.md).

```bash
koan dsp                                        # list profiles; * marks the current output's
koan dsp import "Harman 780.zip"                # Roon zip, .cfg, WAVs, CamillaDSP, APO, AutoEQ…
koan dsp import L48.wav R48.wav --name Room --device "Topping E30"
koan dsp import room.txt --rate 48000           # coefficients that do not say their rate
koan dsp use "Living room" [--device NAME]      # play a device through a profile
koan dsp clear [--device NAME]                  # play a device untouched
koan dsp remove NAME
koan dsp off | on                               # bypass every profile, or stop bypassing
```

`--device` defaults to the current output: `[playback] output_device`, or the
system default.

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

## `koan auth`

Manage authentication -- users, tokens, keypair.

```bash
koan auth setup                       # generate keypair + create first admin user
koan auth create-user --username alice --role user  # create a user (admin, user, readonly)
koan auth delete-user alice           # delete a user
koan auth list-users                  # list all users
koan auth login --server http://localhost:4000 --username admin  # login (stores refresh token in config.local.toml)
koan auth logout --server http://localhost:4000  # logout (revoke token)
koan auth reset-password admin        # reset password (revokes all tokens for that user)
koan auth set-role alice admin        # change role
koan auth api-key create --username alice --name phone  # Subsonic API key, printed once
koan auth api-key list [--username alice]               # keys with created / last used
koan auth api-key revoke 3            # revoke a key by id
koan auth regenerate-keys             # regenerate Ed25519 keypair (invalidates all tokens)
koan auth reset                       # delete all keys, users, tokens
```

Without a terminal, or with `--non-interactive`, `koan auth` never prompts: credentials come from `KOAN_USERNAME` and `KOAN_PASSWORD`, destructive commands need `--yes`, and 1Password is used only with `--save-to-1password`.

```bash
KOAN_USERNAME=admin KOAN_PASSWORD=secret koan auth setup
KOAN_PASSWORD=secret koan auth create-user --username alice --role user
koan auth delete-user alice --yes
```

See [Authentication](../guide/authentication.md) for the full guide.

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

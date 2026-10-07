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
koan play --server http://host:4000          # remote control for a koan server
```

The server plays the audio. To listen on this machine, sign in to the server as a
remote library (`koan remote login`) and play through the local engine. `--jukebox`
is still accepted and changes nothing.

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

Equalisation and convolution, per output device. See
[Equalisation and convolution](../guide/dsp.md).

```bash
koan dsp [show] [DEVICE] [--json]                # what a device plays: the chain in a sentence, then each stage
koan dsp set DEVICE --correction NAME|none --tuning EQ,EQ…|none   # the whole chain in one go; repeating it changes nothing
koan dsp flat [DEVICE]                           # no correction or tuning: plays untouched
koan dsp list [--json]                           # every correction, EQ and preset, and where each is used
koan dsp preset save NAME [--device NAME]        # a device's correction and tuning, saved together
koan dsp preset use NAME|flat [--device NAME]    # set a device from a preset, or flat
koan dsp preset list [--json]                    # the presets, and the devices set from each
koan dsp import "Harman 780.zip"                 # Roon zip, .cfg, WAVs, CamillaDSP, APO, AutoEQ…
koan dsp import L48.wav R48.wav --name Room --device "Topping E30"
koan dsp import room.txt --rate 48000            # coefficients that do not say their rate
koan dsp remove NAME
koan dsp autoeq search QUERY [--limit N] [--refresh]   # AutoEQ results by headphone name, numbered
koan dsp autoeq install NUMBER|NAME [--source SOURCE] [--device NAME]
koan dsp target NAME [--use TARGET | --reset]    # move an AutoEQ correction to another target
koan dsp add-target FILE                         # a target from a CSV or squig.link export
koan dsp measure FILE --name NAME --ear in|over --target TARGET [--fit graphic|squig]  # correct a headphone from its measurement; squig: squig.link's parametric Auto EQ
koan dsp response [DEVICE] [--correction NAME] [--tuning EQ,...] [--rate HZ]  # what the chain plays, as CSV on AutoEQ's grid; nothing saved
koan dsp role NAME correction|tuning|mixed       # mixed: a correction that already includes a tuning
koan dsp made-for NAME TARGET|unknown            # the target a ready-made EQ was made for
koan dsp tuned-for NAME TARGET|unknown           # the target a tuning was made against
koan dsp revert NAME                             # an imported EQ back as imported
koan dsp copy NAME [NEW]                         # a copy as it is now, used by no device
koan dsp split NAME FILE --ear in|over --target TARGET  # a mixed correction into a correction and an EQ
koan dsp squig QUERY [--limit N]                 # measurements on squig.link sites, numbered
koan dsp squig QUERY --use-result N --target TARGET [--ear in|over] [--name NAME] [--fit graphic|squig]  # a correction from one
koan dsp eq plays NAME EQ...                     # one EQ built from others, played in order
koan dsp eq switch NAME EQ on|off                # switch one of the EQs it plays on or off
```

`koan dsp --help` opens with worked examples:

```bash
koan dsp set "Scarlett 4i4 USB" --correction "Wharfedale EVO 4.1" --tuning Lush
koan dsp preset save "Desk" --device "Scarlett 4i4 USB"
koan dsp show "Scarlett 4i4 USB" --json
```

`show --json` gives `device`, `correction`, `target`, `tuning` (each `name`,
`on`, `made_for`, `matched`, `join` and `note`), `preset`, `edited`, `left_out` (EQs that do not play), `notes` (why),
`flat` and `sentence`. A tuning EQ's `matched` is true where it was made
against the correction's target, false where it was made against another or
that is not set, and null where there is nothing to compare; `join.state` is
`matched`, `converted` (with `from` and `to`) or `unknown`, the last meaning a
target may be applied twice. `list --json` gives each one's `name`, `kind`
(`correction`, `eq` or `preset`), `used_on`, `edited`, `members` (a group's)
and `problem` (why it would not load); `preset list --json` gives each
preset's `name`, `used_on` and `edited`. A refusal says what is wrong and the valid
choices ("No correction called X. Corrections: …") and exits non-zero.

The names from before still work, out of the help: `use` (now `set
--correction` or `preset use`), `clear` (`flat`), `tuning` (`set --tuning`),
`stack` and `layer` (`eq plays`, `eq switch`), and `role … baked` (`mixed`).

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

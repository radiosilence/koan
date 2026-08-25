# Configuration Reference

koan uses [figment](https://docs.rs/figment) for layered configuration. Four sources are merged in order -- each layer overrides the one before it:

```
Defaults -> config.toml -> config.local.toml -> KOAN_* env vars
(lowest)                                       (highest priority)
```

| Layer | Path | Purpose |
|-------|------|---------|
| Defaults | (built-in) | Hardcoded sane defaults for every field |
| `config.toml` | `~/.config/koan/config.toml` | Shared settings -- safe to commit to dotfiles |
| `config.local.toml` | `~/.config/koan/config.local.toml` | This machine only, gitignored, `0600` |
| Environment | `KOAN_*` vars | 12-factor overrides -- highest priority, ideal for CI/headless |

Run `koan config` to see all layers and the fully resolved result (including which `KOAN_*` env vars are active).

## Which file a setting goes in

You can put any setting in either file by hand -- the merge does not care. What
the split decides is where *koan* writes when it changes a setting itself, and
that matters because `config.toml` is meant to be committed.

Three kinds of setting are machine-scoped and always land in
`config.local.toml`:

| Kind | Settings |
|------|----------|
| Secrets | `remote.password`, `subsonic.password` |
| This machine's paths, disk, hardware and account | `library.folders`, `remote.enabled/url/username`, `remote.cache_dir`, `remote.cache_limit`, `playback.output_device`, `subsonic.enabled/port/username` |
| Volatile UI state -- flipped by a keypress or a mouse drag | `playback.art_size`, `visualizer.enabled`, `visualizer.mode`, `visualizer.matrix_overlay`, `visualizer.bass_shake` |

Everything else is taste, travels between machines, and goes in `config.toml`.

Writing a setting also clears any copy of it from the other file. That is not
tidiness: `config.local.toml` wins the merge, so a shared write left shadowed by
a local copy would silently do nothing. In the other direction it drains
machine-scoped keys out of the file you commit, which is how a `config.toml`
polluted by an older koan cleans itself up as you use the app.

## Environment variable overrides

Any config field can be overridden via environment variables using the `KOAN_` prefix with `__` (double underscore) as the section separator:

```
KOAN_<SECTION>__<FIELD>=<value>
```

Examples:

```bash
# Remote server password (avoids writing secrets to files)
export KOAN_REMOTE__PASSWORD="hunter2"

# Change GraphQL API port
export KOAN_GRAPHQL__PORT=8080

# Bind API to all interfaces
export KOAN_GRAPHQL__BIND="0.0.0.0"

# Override render FPS
export KOAN_PLAYBACK__TARGET_FPS=30

# Enable the GraphiQL playground
export KOAN_GRAPHQL__PLAYGROUND=true

# Set ReplayGain mode
export KOAN_PLAYBACK__REPLAYGAIN=track
```

Field names match the TOML key in SCREAMING_SNAKE_CASE. Nested sections use `__`:
- `[remote] password` -> `KOAN_REMOTE__PASSWORD`
- `[subsonic] port` -> `KOAN_SUBSONIC__PORT`
- `[playback] pre_amp_db` -> `KOAN_PLAYBACK__PRE_AMP_DB`

## CI usage

Env vars make koan easy to configure in CI without config files:

```yaml
env:
  KOAN_REMOTE__URL: ${{ secrets.NAVIDROME_URL }}
  KOAN_REMOTE__PASSWORD: ${{ secrets.NAVIDROME_PASSWORD }}
  KOAN_GRAPHQL__PORT: 4001
```

## `koan config init`

Creates the config directory at `~/.config/koan/` with everything koan needs to run:

```bash
koan config init
```

What it creates:

| File | Purpose |
|------|---------|
| `config.toml` | Commented template -- all defaults shown as comments for reference, uncomment to customize |
| `config.local.toml` | Template for machine-specific settings (library folders, remote server) |
| `.gitignore` | Ignores `*.log`, `*.db`, `config.local.toml`, `cache/` |
| `koan.db` | SQLite database (created if missing) |
| `cache/` | Download cache directory |

Running `koan config init` on an existing setup is safe -- it merges new defaults without touching values you've changed, and skips `config.local.toml` if it exists.

Machine-scoped settings are left out of the `config.toml` template entirely --
listing them, even commented out, invites them into a dotfiles repo. That means
you can commit `~/.config/koan/` and share playback, visualizer, organize and
radio settings across machines while library paths, credentials and window sizes
stay local.

---

## `[playback]`

```toml
[playback]
replaygain = "off"          # off | track | album
pre_amp_db = 0.0            # dB gain on top of ReplayGain (default: 0.0)
target_fps = 60             # TUI render rate in Hz (default: 60)
show_fps = false            # FPS counter overlay in top-right corner (default: false)

# config.local.toml -- this machine's hardware and window
art_size = 24               # album art width in terminal columns (default: 24)
output_device = "My DAC"    # audio output device name (default: system default)
```

### ReplayGain

ReplayGain normalizes volume levels across tracks so you don't reach for the volume knob between a whisper-quiet jazz track and a wall-of-sound metal album. koan reads standard ReplayGain tags (embedded by tools like `loudgain`, `r128gain`, foobar2000) at decode time and applies gain with peak limiting to prevent clipping.

| Mode | Description |
|------|-------------|
| `off` | No gain adjustment. Original signal untouched |
| `track` | Per-track normalization. Every track plays at the same perceived loudness. Best for shuffled playlists |
| `album` | Per-album normalization. Preserves dynamic range within an album (quiet intros, loud climaxes) while normalizing between albums. **(recommended)** |

`pre_amp_db` adds a fixed gain on top of the ReplayGain adjustment. Positive values make everything louder (risk of clipping), negative values quieter. Useful if your ReplayGain-tagged library feels too quiet at the target level.

### Render FPS

`target_fps` controls how often the TUI redraws. 30, 60, or 120 are typical values. Higher values give smoother visualizer and seek bar updates but use more CPU. Most terminals cap at 60 anyway.

### Album art size

`art_size` sets the width in terminal columns. Height is always `art_size / 2` (square via halfblock rendering, where each cell is 2 pixels tall). The default of 24 columns = 24x12 cells = a 24x24 pixel-equivalent square. Drag the divider under the transport bar to change it; the new size is saved to `config.local.toml`, since it is a property of the terminal you are sitting at.

### Output device

`output_device` selects an audio output by name. Press `Shift+D` in the TUI to browse available devices and switch live. The choice is saved to `config.local.toml` -- your DAC is not the next machine's. If the named device isn't available at startup, koan falls back to the system default.

Run `koan devices` to list available audio outputs.

---

## `[library]`

```toml
# config.local.toml (this machine's paths)
[library]
folders = ["/Volumes/Music/library", "/Users/me/Music"]

# config.toml
[library]
analyze_on_scan = false     # run acoustic analysis on every scan (default: false)
```

One or more directories to scan for music. Subdirectories are scanned recursively.

`analyze_on_scan` computes acoustic features during every `koan scan`, which
roughly doubles it. Off by default -- run `koan scan --analyze` when you want the
features refreshed. Radio mode uses them for "sounds like" matching.

---

## `[remote]`

```toml
# config.local.toml (credentials should stay local)
[remote]
enabled = true
url = "https://music.example.com"
username = "admin"
# password is prompted by `koan remote login` and saved here

# config.toml or config.local.toml
[remote]
download_workers = 5             # parallel download threads (default: 5)
cache_limit = "50GB"             # max cache size, LRU eviction on startup (default: unlimited)
cache_dir = "/custom/path"       # explicit cache dir (default: ~/.config/koan/cache)
```

See [Remote Servers](../guide/remote-servers.md) for the full setup guide.

### Where credentials live

Every secret koan holds -- the remote password, the Subsonic shared secret, the
refresh token for a koan server -- is written to `config.local.toml`, which is
gitignored and created `0600`.

Not the OS keychain, which koan used until v0.31.2. A keychain item's ACL is
keyed on the reading binary's code signature, and koan has no stable signing
identity: ad-hoc signing derives that identity from the binary's own hash, so
every release is a different application to macOS, no grant ever matches twice,
and the password dialog returns on every launch after every update.

The dialog was not buying much in exchange. Subsonic authenticates every request
with the password or a salted MD5 of it, so a client has to keep something
password-equivalent indefinitely -- there is no token to exchange it for, and
Navidrome offers no OAuth. A `0600` file guards it from other accounts on the
machine and from an unencrypted backup, which is the bargain `~/.netrc`,
`~/.aws/credentials` and `gh`'s `hosts.yml` all make.

## `[auth]`

Credentials for a remote koan server *this machine signs in to* -- the other
direction from `[graphql]`, which configures the server koan is.

```toml
# config.local.toml
[auth]
server = "http://localhost:4000"   # written by `koan auth login`
refresh_token = "..."              # exchanged for short-lived access tokens
```

Written by `koan auth login` and cleared by `koan auth logout`, which also
revokes the token at the server. Unlike a password this is revocable, so losing
it costs you one session rather than the account.

See [Authentication](../guide/authentication.md).

---

## `[visualizer]`

```toml
[visualizer]
fps = 60                      # analysis thread update rate in Hz (default: 60)
scale = "bark"                # frequency scale (default: bark)
amplitude_scale = "aweight"   # amplitude scale (default: aweight)
bar_decay_ms = 50             # bar drop half-life in ms (default: 50)
peak_decay_ms = 180           # peak marker linger half-life in ms (default: 180)
palette = "spectrum"          # color palette: spectrum, mono, fire, neon (default: spectrum)
reactivity = 1.0              # animation reactivity 0.0..2.0 (default: 1.0)
reactive_bg = false           # beat-reactive background on braille modes (default: false)

# config.local.toml -- the keybind toggles, saved as you press them
enabled = true                # show visualizer in transport area (default: true)
mode = "bars"                 # visualizer mode (default: bars). Press `v` to pick.
bass_shake = true             # camera jitter on bass hits for braille modes (default: true)
matrix_overlay = false        # replace characters with matrix glyphs (default: false)
```

Also accepts `[visualiser]` spelling.

`enabled`, `mode`, `bass_shake` and `matrix_overlay` have keybinds (`V`, `v`/`M`,
`S`, `M`) and are written back the moment you press one, so they live in
`config.local.toml`. The rest are hand-edited taste and travel with `config.toml`.

22 modes available: bars, oscilloscope, radial, particles, lissajous, spectrogram, stereo waveform, VU meter, flame, plasma, tunnel, wireframe, metaballs, starfield, terrain, moire, kaleidoscope, julia, spiral, interference, wormhole, matrix. Press `v` in the TUI to open the picker with live preview.

The visualizer renders above the transport text when album art is present. 48-band FFT with sub-cell resolution using Unicode block characters, peak hold markers, and smooth exponential decay. The FFT runs on a dedicated thread so the UI is never blocked.

### Frequency scales (`scale`)

Controls how FFT bins map to bars (the X axis):

| Scale | Description |
|-------|-------------|
| `bark` | Bark psychoacoustic scale -- 24 critical bands, matches how your ears group frequencies. Best for music. **(default)** |
| `mel` | Mel perceptual pitch scale -- similar to Bark, widely used in speech/music analysis |
| `log` | Logarithmic -- equal spacing per octave. Familiar if you read spectrograms |
| `linear` | Linear -- equal Hz per bar. Bass is cramped, treble dominates. Analytical use |

### Amplitude scales (`amplitude_scale`)

Controls how magnitudes map to bar height (the Y axis):

| Scale | Description |
|-------|-------------|
| `aweight` | A-weighted (IEC 61672). Reflects perceived loudness -- bass and extreme treble attenuated to match human hearing. **(default)** |
| `perceptual` | A-weighting + gentle gamma curve. Same frequency correction with a boost to quiet signals |
| `sqrt` | Square root curve -- gentle boost to quiet bands, no frequency correction |
| `linear` | Raw dB-normalized magnitude. No correction. Technically accurate |

---

## `[organize]`

```toml
[organize]
default = "standard"      # pattern selected by default in the TUI modal

[organize.patterns]
standard = "%album artist%/(%date%) %album%/%tracknumber%. %title%"
va-aware = "%album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%"
flat = "%artist% - %title%"
```

Named patterns used by the TUI organize modal. Format strings use fb2k syntax -- `%field%` for metadata, `$function()` for transforms, `[conditionals]` to omit blocks when fields are missing. See [Format Strings](../format-strings.md) for the full reference.

The `va-aware` pattern handles compilations: if the album artist is "Various Artists", it includes the per-track artist in the filename and omits the redundant year prefix.

Files are organized into the **first configured library folder** (from `[library] folders`). The format pattern generates the relative path within that folder.

See [File Organization](../guide/file-organization.md) for a walkthrough.

---

## `[graphql]`

```toml
[graphql]
enabled = true                # run API alongside TUI (default: true, false = --no-api)
port = 4000                   # API port (default: 4000)
bind = "127.0.0.1"            # bind address (default: 127.0.0.1)
playground = false            # enable GraphiQL IDE at GET /graphql (default: false)
auth_enabled = true           # JWT authentication (default: true)
access_token_ttl = "15m"      # access token lifetime (default: 15m)
refresh_token_ttl = "30d"     # refresh token lifetime (default: 30d)
cors_origins = []             # origins allowed to call the API from a browser
allowed_hosts = []            # extra Host: values to answer to (see below)
cookie_secure = false         # mark cookies Secure — only with HTTPS in front
allow_organize = false        # expose the organize* mutations, which move files
```

Auth is enabled by default. Run `koan auth setup` to create a keypair and admin user. Set `auth_enabled = false` if you only use localhost and don't need auth.

### Browser access

`cors_origins` is empty by default, which means no web page may read the API cross-origin. Add the origin your web client is served from — `["https://music.example.com"]` — to allow it.

`allowed_hosts` names the hostnames this server answers to, on top of `localhost` and any bare IP address. A request arriving with any other `Host` is refused: without that check, a page whose DNS flips to `127.0.0.1` after loading reaches the API as same-origin and CORS stops applying. Set it if you reach koan through a name like `koan.lan`.

`cookie_secure` should stay `false` unless clients reach koan over HTTPS. Browsers discard `Secure` cookies delivered over plain `http://` to anything but localhost, so setting it on a LAN deployment silently breaks cookie auth.

`allow_organize` gates `organizePreview`, `organizeExecute` and `organizeUndo`. They rename and move files on disk, which is not something a network API should offer by default.

---

## `[subsonic]`

koan's own Subsonic-compatible REST API, served at `/rest/*`.

```toml
# config.local.toml -- which machine serves Subsonic, and as whom
[subsonic]
enabled = false               # serve /rest/* on the GraphQL port (default: false)
port = 4040                   # also serve it on a dedicated port (default: none)
username = "koan"             # username Subsonic clients authenticate as
```

`enabled` mounts `/rest/*` on the GraphQL port. `port` adds a second listener for
clients that insist on Subsonic having one of its own.

Run `koan subsonic setup` to enable it. That generates a secret, writes it to `config.local.toml`, and prints it once.

The secret is deliberately **not** your Navidrome/`[remote]` password. The Subsonic protocol authenticates with `md5(secret + salt)` where the client picks the salt, over whatever transport it likes — so anyone who can capture one request walks away with a digest to crack offline. A generated 256-bit secret makes that worthless; your Navidrome password would not.

`/rest/*` is not covered by JWT auth, so these credentials alone guard every file in the library. Don't expose the port to the internet.

See [Authentication](../guide/authentication.md), [GraphQL API](../guide/graphql-api.md), and [Headless Server](../guide/headless-server.md) for usage guides.

---

## `[radio]`

```toml
[radio]
lookahead = 5                 # tracks to keep queued ahead (default: 5)
batch_size = 5                # tracks added per refill (default: 5)
history_window = 200          # don't repeat last N tracks (default: 200)
seed_window = 5               # recent tracks used as seed for similarity (default: 5)
discovery_weight = 0.3        # 0.0 = familiar only, 1.0 = maximize discovery (default: 0.3)
```

See [Radio Mode](../guide/radio-mode.md) for a full guide.

---

## File paths

| File | Default location |
|------|-----------------|
| Config (base) | `~/.config/koan/config.toml` |
| Config (local) | `~/.config/koan/config.local.toml` |
| Database | `~/.config/koan/koan.db` |
| Download cache | `~/.config/koan/cache/` |
| Log file | `~/.config/koan/koan.log` |

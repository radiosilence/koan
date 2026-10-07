# Configuration

Four sources are merged, each overriding the one before it:

```
Defaults -> config.toml -> config.local.toml -> KOAN_* env vars
(lowest)                                       (highest priority)
```

| Layer | Path | Purpose |
|-------|------|---------|
| Defaults | (built-in) | |
| `config.toml` | `~/.config/koan/config.toml` | Shared settings -- safe to commit to dotfiles |
| `config.local.toml` | `~/.config/koan/config.local.toml` | This machine only, gitignored, `0600` |
| Environment | `KOAN_*` vars | Containers and one-off overrides |

Run `koan config` to see the merged result, the files it came from and which `KOAN_*` env vars are active.

## Which file a setting goes in

You can put any setting in either file by hand -- the merge does not care. What
the split decides is where *kōan* writes when it changes a setting itself, and
that matters because `config.toml` is meant to be committed.

Three kinds of setting are machine-scoped and always land in
`config.local.toml`:

| Kind | Settings |
|------|----------|
| Secrets | `remote.password`, `subsonic.password` |
| This machine's paths, disk, hardware and account | `library.folders`, `remote.enabled/url/username`, `remote.cache_dir`, `remote.cache_limit`, `playback.output_device/renderers/muted`, `subsonic.enabled/port/username/transcode/ffmpeg`, `devices.nearby/discoverable/port/addresses/nearby_control/refused/keep_running`, everything under `dsp` |
| Volatile UI state -- flipped by a keypress or a mouse drag | `playback.art_size`, `visualizer.enabled`, `visualizer.mode`, `visualizer.matrix_overlay`, `visualizer.bass_shake` |

Everything else is taste, travels between machines, and goes in `config.toml`.

Writing a setting also clears any copy of it from the other file, because
`config.local.toml` wins the merge: a shared write left shadowed by a local copy
would silently do nothing. In the other direction it drains
machine-scoped keys out of the file you commit, which is how a `config.toml`
polluted by an older kōan cleans itself up as you use the app.

## What is not in the config files

The macOS app's own view state -- whether the lyrics panel is open, whether the
queue is grouped, how much the app draws -- lives in macOS defaults
(`defaults read cc.blit.koan`), not in `config.toml`. None of it means anything
to the CLI or the TUI, and a setting that travels between machines in a
committed dotfile should be one that makes sense on all of them.

The graphics level is the one worth knowing about. Settings -> Appearance, or:

```bash
defaults write cc.blit.koan graphics -int 0   # 0 plain, 1 reduced, 2 full
```

| | Wash | Indicators | Chrome | Cost |
|---|---|---|---|---|
| `2` Full (default) | drifts | dance | glass | 15-18% of a core |
| `1` Reduced | held still | dance | glass | ~9% |
| `0` Plain | none | held still | flat materials | ~6% |

Measured on an M1 Pro, playing, window frontmost. The wash's blur is close to
free -- it is rasterised once and magnified as a texture -- so holding it still
costs about what not drawing it costs. It is the drift that is expensive, which
is why `Reduced` keeps the record's colour and only stops it moving.

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

## `koan config init`

Creates the config directory at `~/.config/koan/` with everything kōan needs to run:

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
you can commit `~/.config/koan/` and share playback, visualizer and organize
settings across machines while library paths, credentials and window sizes
stay local.

---

## `[playback]`

```toml
[playback]
replaygain = "off"          # off | track | album
pre_amp_db = 0.0            # dB gain on top of ReplayGain (default: 0.0)
fade_on_pause = true        # fade out on pause, back in on resume (default: true)
rate_switch_lead_in_ms = 1000 # silence after a sample rate change (default: 1000)
target_fps = 60             # TUI render rate in Hz (default: 60)
show_fps = false            # FPS counter overlay in top-right corner (default: false)

# config.local.toml -- this machine's hardware and window
art_size = 24               # album art width in terminal columns (default: 24)
output_device = "My DAC"    # audio output device name (default: system default)
renderers = true            # look for UPnP renderers on the network (default: true)
muted = false               # play silence (default: false)
```

### ReplayGain

ReplayGain normalizes loudness across tracks. kōan reads standard ReplayGain tags (embedded by tools like `loudgain`, `r128gain`, foobar2000) at decode time and applies gain with peak limiting to prevent clipping.

| Mode | Description |
|------|-------------|
| `off` | No gain adjustment. Original signal untouched |
| `track` | Per-track normalization. Every track plays at the same perceived loudness. Best for shuffled playlists |
| `album` | Per-album normalization. Preserves dynamic range within an album (quiet intros, loud climaxes) while normalizing between albums. **(recommended)** |

`pre_amp_db` adds a fixed gain on top of the ReplayGain adjustment. Positive values make everything louder (risk of clipping), negative values quieter. Useful if your ReplayGain-tagged library feels too quiet at the target level.

### Fade on pause

With `fade_on_pause`, pause ramps the output down over 150ms before the audio unit stops, and resume ramps it back up. The ramp is applied in the render callback and only while it runs; at full level samples are copied unmodified. The position rests on the last sample that was audible, not on audio consumed during the fade and discarded. Off, pause and resume cut immediately.

### Rate switch lead-in

When a track needs the output device at a different sample rate, the device relocks its clock, and many interfaces mute until it has. koan cannot see that happen: CoreAudio reports the new rate as soon as it is accepted, and no property says when the clock has locked. Audio sent in that window is lost, so the start of the track goes missing. `rate_switch_lead_in_ms` plays that much silence first, through the running output so devices that only relock on a live stream do so, and the playhead stays at the start until the track itself is heard. Only a rate change adds it; a resume clears it. How long the relock takes depends on the device, so the setting lives in `config.local.toml`. 0 turns it off.

### Render FPS

`target_fps` controls how often the TUI redraws. 30, 60, or 120 are typical values. Higher values give smoother visualizer and seek bar updates but use more CPU.

### Album art size

`art_size` sets the width in terminal columns. Height is always `art_size / 2` (square via halfblock rendering, where each cell is 2 pixels tall). The default of 24 columns = 24x12 cells = a 24x24 pixel-equivalent square. Drag the divider under the transport bar to change it; the new size is saved to `config.local.toml`, since it is a property of the terminal you are sitting at.

### Output device

`output_device` selects an audio output by name. Press `Shift+D` in the TUI to browse available devices and switch live. The choice is saved to `config.local.toml` -- your DAC is not the next machine's. If the named device isn't available at startup, kōan falls back to the system default.

Run `koan devices` to list available audio outputs.

### Automated runs

`muted` zeroes every sample in the render callback, so playback carries on, with its position, queue and gapless handover, but nothing is heard. `renderers = false` stops kōan looking for UPnP renderers, so none appears under Output and none can be played to. The app UI tests set both, as `KOAN_PLAYBACK__MUTED` and `KOAN_PLAYBACK__RENDERERS`, because they run on a machine someone is using, whose speakers and network renderers are theirs.

---

## `[appearance]`

How the macOS, iOS and tvOS apps are drawn. The theme is read once as an app
opens, so a change of theme shows the next time it starts; the others apply at
once.

```toml
[appearance]
theme = "koan"     # "koan": the site's look throughout (the default); "system": the platform's own
theme_icons = true # in the kōan theme, icons beside labels; false for labels alone
record_colours = true # the record playing colours the wash and the accent; false for neither
wash_window = true    # provisional. Mac, kōan theme: the wash under the whole window; false gives panels their own grounds
```

Settings → Appearance → **Theme** chooses between them on every app, and
**Show icons**, shown while the kōan theme is chosen, sets `theme_icons`. The
system look always draws its icons. **Colours from the record** sets
`record_colours`: off, there is no wash behind the window and the accent is
koan's mint, in either theme. `wash_window` (the Mac, kōan theme only) is provisional,
there to live with both looks while one is chosen, and likely to go: on, the
sidebar, toolbar, transport and lyrics are drawn clear over one wash; off, they
keep grounds of their own. The graphics level is separate, and governs what the
wash costs rather than whether it takes colour; the playing bars move at every
level, held still only by Reduce Motion. The theme's design is
`docs/design/koan-theme.md`.

## `[library]`

```toml
# config.local.toml (this machine's paths)
[library]
folders = ["/Volumes/Music/library", "/Users/me/Music"]
```

One or more directories to scan for music. Subdirectories are scanned recursively.

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
cache_limit = "50GB"             # max cache size, LRU eviction at startup and as downloads land (default: unlimited)
cache_dir = "/custom/path"       # explicit cache dir (default: ~/.config/koan/cache)
```

See [Remote Servers](../guide/remote-servers.md) for the full setup guide.

### Where credentials live

Every secret kōan holds -- the remote password, the Subsonic shared secret, the
refresh token for a kōan server -- is written to `config.local.toml`, which is
gitignored and created `0600`.

Not the OS keychain: a keychain item is bound to the reading binary's code signature, which changed with every build, so macOS asked for the password after every update. A Subsonic client has to keep something password-equivalent in any case, and a `0600` file is the same bargain `~/.netrc` and `gh`'s `hosts.yml` make.

## `[auth]`

Credentials for a remote kōan server *this machine signs in to* -- the other
direction from `[graphql]`, which configures the server kōan is.

```toml
# config.local.toml
[auth]
server = "http://localhost:4000"   # written by `koan auth login`
refresh_token = "..."              # exchanged for short-lived access tokens
```

Written by `koan auth login` and cleared by `koan auth logout`, which also
revokes the token at the server. `koan play --server` reads it, and rewrites
`refresh_token` each time it refreshes, since the server revokes the old one. Unlike a password this is revocable, so losing
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
`S`, `X`) and are written back the moment you press one, so they live in
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

Named patterns for organize, in [format string](../format-strings.md) syntax. See [File organization](../guide/file-organization.md).

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
proxy_auth_header = ""        # header an authenticating proxy names the user in
proxy_auth_from = []          # addresses or ranges that proxy connects from
allow_organize = false        # expose the organize* mutations, which move files
```

Auth is enabled by default. Run `koan auth setup` to create a keypair and admin user. Set `auth_enabled = false` if you only use localhost and don't need auth.

### Browser access

`cors_origins` is empty by default, which means no web page may read the API cross-origin. Add the origin your web client is served from — `["https://music.example.com"]` — to allow it.

`allowed_hosts` names the hostnames this server answers to, on top of `localhost` and any bare IP address. A request arriving with any other `Host` is refused: without that check, a page whose DNS flips to `127.0.0.1` after loading reaches the API as same-origin and CORS stops applying. Set it if you reach kōan through a name like `koan.lan`.

`cookie_secure` should stay `false` unless clients reach kōan over HTTPS. Browsers discard `Secure` cookies delivered over plain `http://` to anything but localhost, so setting it on a LAN deployment silently breaks cookie auth.

`proxy_auth_header` and `proxy_auth_from` sign the web UI in through an authenticating reverse proxy. Both are set or neither: the server refuses to start with only one, with an entry it cannot parse, or with a range covering every address. See [Behind an authenticating proxy](../guide/headless-server.md#behind-an-authenticating-proxy).

`allow_organize` gates `organizePreview`, `organizeExecute` and `organizeUndo`. They rename and move files on disk, which is not something a network API should offer by default.

---

## `[subsonic]`

kōan's Subsonic API, served at `/rest/*`. Clients sign in with a kōan account; see [Authentication](../guide/authentication.md#subsonic-api).

```toml
# config.local.toml -- which machine serves Subsonic
[subsonic]
enabled = false               # serve /rest/* on the main port (default: false)
port = 4040                   # also serve it on a port of its own (default: none)
username = "koan"             # the shared secret's username (default: koan)
transcode = true              # transcode stream for clients that ask (default: true)
ffmpeg = "ffmpeg"             # the ffmpeg transcoding runs, on PATH or a path (default: ffmpeg)
```

`koan subsonic setup` enables it and generates a shared secret, written to `config.local.toml` and printed once. The secret signs in as `username` with `user` rights, for a client that has no account of its own; `koan play --server` streams with it. It is generated rather than chosen because Subsonic token auth sends `md5(secret + salt)` with every request, and a captured digest of a human-chosen password can be cracked offline.

A client that asks `stream` for a `maxBitRate` below the file's bitrate, or for `format=opus`, `format=mp3` or `format=aac` (also `m4a`), gets a transcode made by `ffmpeg`: Opus unless another format was asked for, at the requested bitrate or 128 kbps (Opus) and 192 kbps (MP3, AAC). AAC is AAC-LC from ffmpeg's built-in encoder, sent as ADTS (`audio/aac`); a file already in the requested format and within the limit is sent as it is. `format=raw`, and `download`, always return the original. A transcode has no length until it ends, so it is sent without one, unless the client passes `estimateContentLength=true` (the body is then cut or padded to the bitrate times the duration), and without Range support; clients seek with `timeOffset`, offered as the OpenSubsonic `transcodeOffset` extension. The server runs at most one transcode per CPU core and three per account; beyond that, and whenever ffmpeg produces nothing, the original is served. Where `ffmpeg` does not run or has none of `libopus`, `libmp3lame` and `aac`, the server logs it once at startup and serves originals. The container image includes it.

---

## `[sharing]`

```toml
[sharing]
public_url = "https://music.example.com"
```

The address the server is reached at from outside. Share links, invite links and MCP sign-in are built on it; without it the server makes no share links and offers MCP clients no sign-in, since an address taken from request headers could be chosen by whoever sends them.

---

## `[mcp]`

```toml
[mcp]
redirect_hosts = ["claude.ai", "claude.com"]
```

The hosts an MCP client may register to return to, besides this machine. Empty allows any HTTPS host, and each approval rests on the person recognising the host the consent page names. See [MCP integration](../guide/mcp-integration.md).

---

## `[push]`

Push notifications to kōan's iOS app, for a server whose linked phones should
stay reachable once iOS has suspended the app.

```toml
# config.local.toml -- the APNs key is a secret
[push]
key_path = "/path/to/AuthKey_XXXXXXXXXX.p8"  # or `key` (KOAN_PUSH__KEY) with the PEM itself
key_id = "XXXXXXXXXX"
team_id = "XXXXXXXXXX"
topic = "cc.blit.koan"                       # the app's bundle id (default)
```

A linked app is reached over its WebSocket. iOS suspends a backgrounded app
that is not playing, and the socket goes with it. With a key configured, the
server then sends the app a background push, which wakes it to link and take
what waits in its outbox (syncs, evictions), and turns a request to play on it
into a notification the person taps: iOS does not let a suspended app start
playing on its own. GraphQL's `playOnClient` says which happened.

With `sharing.public_url` set, that notification shows the album's cover. It
carries a link to `/push/cover/…` that opens that one cover for ten minutes,
signed with a key the server mints at start-up and never stores, so the app's
notification extension can fetch it without a login.

Apple accepts pushes only signed with the key of the team that ships the app,
so only a server holding that key can send them. Without one, phones are
reached only while linked. Development builds of the app use
Apple's sandbox gateway and release builds the production one; the app says
which with its token, and the key works for both.

---

## `[devices]`

Controlling this device from koan apps on the same network, and reaching
devices on networks that do not announce them. Devices on the same account
reach each other through the server whatever this says. See
[Playing on another device](../guide/devices.md).

```toml
# config.local.toml -- whether a machine is open to its network is its own business
[devices]
nearby = true                       # take part in the local network at all; false reaches others only through the server
discoverable = true                 # listen, and announce this device (and its server's address) over Bonjour
port = 5626                         # fixed, so a typed address keeps working
addresses = ["mac-mini:5626"]       # dialled directly: for a tailnet, which carries no Bonjour
nearby_control = "full"             # "full" or "playback": what devices on the network may do here
refused = ["192.168.1.40"]          # addresses whose connections are hung up at once; Settings → Devices → Refuse
keep_running = false                # the Mac app stays in the menu bar with its window closed
```

A discoverable device can be seen and controlled by any koan app on the
network, whoever is signed in there, never with this device's account's
powers: nothing reaches the library, the files on disk, or the account's
playlists, favourites or history. `nearby_control = "full"`, the default, is
for a household network: they may also choose the output, the preset and the
volume, and move the music here or away. `"playback"` is for a network shared
with strangers: play and the queue only, and a hand-off that stays on the
network. If the port is taken, koan listens on another and announces that one,
so only typed addresses miss it.

---

## `[dsp]`

Corrections, EQs and presets, each correction for the output devices it names.
They describe the listening setup, so all of `[dsp]` belongs to the machine. See
[Equalisation and convolution](../guide/dsp.md).

```toml
# config.local.toml
[dsp]
enabled = true                      # false, from before Flat, makes every device flat at start

[[dsp.profiles]]
name = "Living room"
devices = ["Topping E30"]           # as `koan devices` names them
impulses = ["dsp/living-room/48000.wav"]  # WAV or Convolver .cfg per rate; relative to this directory
# preamp_db = -6.0                  # unset: derived from the filters' peak gain
filters = [
    { type = "peaking", freq = 20.0, gain_db = -1.3, q = 2.0 },
    { type = "gain", freq = 1000.0, gain_db = -2.0, channels = [1] },  # channels from 0; unset is all
]
```

They are normally made by importing (`koan dsp import`, or Settings in the
apps), which reads every format the guide lists and keeps the result under
`dsp/` here.

---

## File paths

| File | Default location |
|------|-----------------|
| Config (base) | `~/.config/koan/config.toml` |
| Config (local) | `~/.config/koan/config.local.toml` |
| Database | `~/.config/koan/koan.db` |
| Download cache | `~/.config/koan/cache/` |
| Log file | `~/.config/koan/koan.log` |

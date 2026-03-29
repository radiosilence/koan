# Configuration Reference

koan uses [figment](https://docs.rs/figment) for layered configuration. Four sources are merged in order -- each layer overrides the one before it:

```
Defaults -> config.toml -> config.local.toml -> KOAN_* env vars
(lowest)                                       (highest priority)
```

| Layer | Path | Purpose |
|-------|------|---------|
| Defaults | (built-in) | Hardcoded sane defaults for every field |
| `config.toml` | `~/.config/koan/config.toml` | Shareable base config -- safe to commit to dotfiles |
| `config.local.toml` | `~/.config/koan/config.local.toml` | Machine-specific paths, credentials (gitignored) |
| Environment | `KOAN_*` vars | 12-factor overrides -- highest priority, ideal for CI/headless |

Run `koan config` to see all layers and the fully resolved result (including which `KOAN_*` env vars are active).

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
- `[graphql] subsonic_port` -> `KOAN_GRAPHQL__SUBSONIC_PORT`
- `[playback] pre_amp_db` -> `KOAN_PLAYBACK__PRE_AMP_DB`

## CI usage

Env vars make koan easy to configure in CI without config files:

```yaml
env:
  KOAN_REMOTE__URL: ${{ secrets.NAVIDROME_URL }}
  KOAN_REMOTE__PASSWORD: ${{ secrets.NAVIDROME_PASSWORD }}
  KOAN_GRAPHQL__PORT: 4001
```

## `koan init`

Creates the config directory at `~/.config/koan/` with everything koan needs to run:

```bash
koan init
```

What it creates:

| File | Purpose |
|------|---------|
| `config.toml` | Base config with all defaults (new fields are merged in without overwriting your customizations) |
| `config.local.toml` | Template for machine-specific settings (library folders, remote server) |
| `.gitignore` | Ignores `*.log`, `*.db`, `config.local.toml`, `cache/` |
| `koan.db` | SQLite database (created if missing) |
| `cache/` | Download cache directory |

Running `koan init` on an existing setup is safe -- it merges new defaults without touching values you've changed, and skips `config.local.toml` if it exists.

`library.folders` is deliberately excluded from `config.toml` (it's machine-specific and belongs in `config.local.toml`). This means you can commit `~/.config/koan/` to your dotfiles repo and share playback/visualizer/organize settings across machines while keeping library paths and credentials local.

---

## `[playback]`

```toml
[playback]
software_volume = false     # volume control in software (vs hardware/DAC)
replaygain = "album"        # off | track | album
pre_amp_db = 0.0            # dB gain on top of ReplayGain (default: 0.0)
target_fps = 60             # TUI render rate in Hz (default: 60)
ticker_fps = 8              # title scroll speed in Hz (default: 8)
show_fps = false            # FPS counter overlay in top-right corner (default: false)
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

### Ticker

When the artist/title text overflows the available transport bar width, it scrolls horizontally like a ticker. `ticker_fps` controls the scroll speed -- one character per frame. Higher values scroll faster.

### Album art size

`art_size` sets the width in terminal columns. Height is always `art_size / 2` (square via halfblock rendering, where each cell is 2 pixels tall). The default of 24 columns = 24x12 cells = a 24x24 pixel-equivalent square.

### Output device

`output_device` selects an audio output by name. Press `Shift+D` in the TUI to browse available devices and switch live. The choice is persisted to config. If the named device isn't available at startup, koan falls back to the system default.

Run `koan devices` to list available audio outputs.

---

## `[library]`

```toml
# config.local.toml (machine-specific)
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
transcode_quality = "original"   # original | opus-128 | mp3-320 (default: original)
download_workers = 5             # parallel download threads (default: 5)
cache_limit = "50GB"             # max cache size, LRU eviction on startup (default: unlimited)
cache_dir = "/custom/path"       # explicit cache dir (default: ~/.config/koan/cache)
```

See [Remote Servers](../guide/remote-servers.md) for the full setup guide.

---

## `[visualizer]`

```toml
[visualizer]
enabled = true                # show spectrum analyzer in transport area (default: true)
fps = 60                      # analysis thread update rate in Hz (default: 60)
scale = "bark"                # frequency scale (default: bark)
amplitude_scale = "aweight"   # amplitude scale (default: aweight)
bar_decay_ms = 50             # bar drop half-life in ms (default: 50)
peak_decay_ms = 180           # peak marker linger half-life in ms (default: 180)
```

Also accepts `[visualiser]` spelling.

The spectrum analyzer renders above the transport text when album art is present. 48-band FFT with sub-cell resolution using Unicode block characters, peak hold markers, and smooth exponential decay. Bars are colored by signal level -- green at safe headroom, yellow when getting hot, red near clipping (0dBFS). The FFT runs on a dedicated thread so the UI is never blocked.

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
playground = false             # enable GraphiQL IDE at GET /graphql (default: false)
subsonic_port = 4040           # optional Subsonic REST API port (default: disabled)
```

Set `bind = "0.0.0.0"` to listen on all interfaces. There's no authentication, so only do this on trusted networks.

See [GraphQL API](../guide/graphql-api.md) and [Headless Server](../guide/headless-server.md) for usage guides.

---

## `[radio]`

```toml
[radio]
lookahead = 5                 # tracks to keep queued ahead (default: 5)
batch_size = 5                # tracks added per refill (default: 5)
use_subsonic = true           # use Subsonic similarity when available (default: true)
history_window = 200          # don't repeat last N tracks (default: 200)
seed_window = 5               # recent tracks used as seed for similarity (default: 5)
discovery_weight = 0.3        # 0.0 = familiar only, 1.0 = maximize discovery (default: 0.3)
```

See [Radio Mode](../guide/radio-mode.md) for a full guide.

---

## `[discovery]`

```toml
[discovery]
analysis_on_scan = false      # run acoustic analysis during library scan (default: false)
acoustic_weight = 0.5         # weight of acoustic similarity in radio scoring 0.0..1.0 (default: 0.5)
```

Run `koan scan --analyze` to compute acoustic features for your library. Higher `acoustic_weight` gives radio mode more "sounds like" awareness vs. metadata-based matching.

---

## File paths

| File | Default location |
|------|-----------------|
| Config (base) | `~/.config/koan/config.toml` |
| Config (local) | `~/.config/koan/config.local.toml` |
| Database | `~/.config/koan/koan.db` |
| Download cache | `~/.config/koan/cache/` |
| Log file | `~/.config/koan/koan.log` |

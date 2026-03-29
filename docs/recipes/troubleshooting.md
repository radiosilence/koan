# Troubleshooting

Common issues and how to fix them.

## Audio

### No sound / wrong output device

1. Check which devices koan sees:
   ```bash
   koan devices
   ```
2. Set the correct device in config:
   ```toml
   [playback]
   output_device = "Your DAC Name"
   ```
   Or press `Shift+D` in the TUI to switch live.

3. If the device name changed (e.g. after a macOS update), koan falls back to the system default. Update the config or re-select in the TUI.

### Sample rate mismatch / clicks / pops

koan switches the audio device's sample rate to match the source file. If you hear artifacts:

- Check that your DAC supports the source sample rate (`koan probe <file>` to check)
- Some USB DACs need a moment to lock onto a new sample rate -- this is normal for the first ~100ms of a rate-switched track
- If using AirPlay or Bluetooth, sample rate switching isn't supported -- koan will play at whatever rate the device is already set to

### Port already in use (GraphQL API)

```
WARN: Failed to bind to 127.0.0.1:4000
```

Another koan instance (or another process) is using port 4000. Either:
- Kill the other process: `lsof -i :4000`
- Use a different port: `koan --port 8080` or `KOAN_GRAPHQL__PORT=8080`

## Library

### Scan doesn't find my files

- Check that `[library] folders` in `config.local.toml` points to the right directory
- koan scans recursively -- you only need the top-level directory
- Supported formats: FLAC, MP3, AAC, Vorbis, Opus, ALAC, WavPack, WAV, AIFF, APE, M4A
- Run `koan config` to verify the resolved config

### Duplicate tracks after remote sync

koan deduplicates using artist + album + title + track number. If you see duplicates:
- Tags might differ between local files and the remote server (e.g. different artist spelling)
- Run `koan scan` after `koan remote sync` to re-merge

### Search returns nothing

Make sure you've run `koan scan` at least once. The FTS5 index is built during scanning.

## Remote

### Connection refused / timeout

```bash
koan remote status    # check connection
```

- Verify the URL is correct (include `https://` or `http://`)
- Check that the Subsonic API is enabled on your server (Navidrome: Settings -> Subsonic)
- Try the URL in a browser to verify the server is reachable

### Authentication failed

```bash
koan remote login https://music.example.com admin
```

Re-run login to update credentials. koan uses MD5+salt authentication (standard Subsonic protocol). Some servers require enabling "legacy authentication" in their settings.

### Sync stalls or is very slow

The first sync fetches all albums and tracks. For large libraries (50k+ tracks), this can take several minutes.

- Progress is displayed during sync
- Subsequent syncs are incremental (much faster)
- Check your server's rate limits if sync seems throttled

## TUI

### Album art not showing

Album art uses halfblock rendering (Unicode block characters). Requirements:
- Terminal must support Unicode
- Terminal must support 256 colors or truecolor
- Art is extracted from embedded tags (FLAC, MP3, etc.) or downloaded from the remote server

If art shows as garbled characters, your terminal may not support halfblock rendering. Try a different terminal (iTerm2, Ghostty, WezTerm, Kitty all work well).

### Visualizer not showing

The spectrum analyzer renders in the transport area when album art is present. If it's not visible:
- Check that `[visualizer] enabled = true` (default)
- The terminal window needs to be wide enough to fit both album art and the visualizer
- Some terminals with limited Unicode support may not render the block characters correctly

### Terminal not restored after crash

If koan crashes and leaves your terminal in a bad state (no echo, wrong colors):

```bash
reset
```

koan installs a panic hook that attempts to restore the terminal on any thread, but some crash modes (SIGKILL, OOM) bypass the hook.

## Config

### Changes not taking effect

Config is loaded at startup. Restart koan after editing config files.

Check the resolved config to verify your changes are being picked up:
```bash
koan config
```

Environment variables (`KOAN_*`) override file config. If a value isn't what you expect, check for conflicting env vars.

### Secrets appearing in config.toml

If sensitive values from `config.local.toml` or environment variables appear in `config.toml`, something called `save()` on a merged config instead of using `Config::update_base()`. This is a bug -- please report it.

## Logs

koan logs to `~/.config/koan/koan.log`. Check this file for detailed error messages when something goes wrong. In daemon mode (`-d`), this is the primary debugging tool.

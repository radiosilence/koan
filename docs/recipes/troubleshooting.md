# Troubleshooting


## Audio

### No sound / wrong output device

1. Check which devices kōan sees:
   ```bash
   koan devices
   ```
2. Set the correct device in config:
   ```toml
   [playback]
   output_device = "Your DAC Name"
   ```
   Or press `D` in the TUI to switch live.

3. If the device name changed (e.g. after a macOS update), kōan falls back to the system default. Update the config or re-select in the TUI.

### Sample rate mismatch / clicks / pops

kōan switches the audio device's sample rate to match the source file. If you hear artifacts:

- Check that the DAC supports the file's sample rate; `koan probe <file>` shows it.
- If the start of a track goes missing after a rate change, the device is slow to relock its clock. Raise `playback.rate_switch_lead_in_ms` (see [Configuration](../reference/configuration.md#rate-switch-lead-in)).
- AirPlay and Bluetooth devices do not switch rate; kōan plays at the rate they are set to.

### Port already in use (GraphQL API)

```
WARN: Failed to bind to 127.0.0.1:4000
```

Another kōan instance (or another process) is using port 4000. Either:
- Find the other process: `lsof -i :4000`
- Use a different port: `koan --port 8080` or `KOAN_GRAPHQL__PORT=8080`

The TUI serves the API only with `--api`, `--port`, `--bind`, `--subsonic`, `--playground` or `[graphql] enabled = true`; `koan --no-api` starts it without, whatever the config says.

### A server will not start: server.url is not set

`koan --headless` needs `server.url` (or `KOAN_SERVER__URL`), the `http://` or `https://` address clients reach it at. Share links, invites, MCP's sign-in and push covers are built on it. A server that had `sharing.public_url` keeps working with a warning until `server.url` is set; see [Configuration](../reference/configuration.md#server).

## Library

### Scan doesn't find my files

Check that `[library] folders` names the top-level directory (scans are recursive) and that `koan config` shows it. Supported formats are FLAC, MP3, AAC, Vorbis, Opus, ALAC, ADPCM, WAV, AIFF and CAF, in Ogg, Matroska/WebM and MP4 containers.

### Duplicate tracks after remote sync

kōan merges a local file with the server's copy when their album, album artist, disc, track number and title agree (the track artist breaks a tie), or when both carry the same MusicBrainz recording and release ids. If you see duplicates:
- Tags might differ between local files and the remote server (e.g. a different album or album artist spelling, or a missing track number)
- Fix the tags, then `koan scan --force` — a plain scan skips files whose mtime and size have not changed, and it is the re-read that spots the merge
- In the apps, Info… on a track's menu shows what each source says; signed in to a kōan server as an admin, it also shows the file's path on the server, which says where a duplicate comes from

### Search returns nothing

The search index is built by `koan scan`; run it at least once.

## Remote

### Connection refused / timeout

```bash
koan remote status    # check connection
```

Check that the URL includes `https://` or `http://` and opens in a browser. On a kōan server, the Subsonic API has to be enabled with `koan subsonic setup`.

### Authentication failed

```bash
koan remote login https://music.example.com admin
```

Re-run login to update the stored password. kōan signs in with Subsonic token auth (salted MD5).

### Sync stalls or is very slow

A sync walks the whole library, which takes minutes for 50,000 tracks or more. The automatic syncs skip the walk when the server reports no change, so only the first one and forced ones are slow.

## TUI

### Album art not showing

Where the terminal supports the kitty or iTerm2 graphics protocol, covers are drawn as images; elsewhere they are Unicode half blocks in truecolor or 256 colours. `I` switches between the two, and `playback.art_graphics = false` keeps half blocks. Garbled half blocks mean the terminal or its font lacks one of those.

### Visualizer not showing

The visualizer draws in the transport area beside the album art, so it needs a window wide enough for both. `V` opens the picker, whose first choice is off, and `F` shows the visualizer full screen.

### Terminal not restored after crash

Run `reset`. kōan restores the terminal on a panic, but not when it is killed outright.

## Config

### Changes not taking effect

The TUI and server read config at startup, so restart after editing. `koan config` shows the merged result and any `KOAN_*` environment variables, which override both files.

### Secrets appearing in config.toml

Every setting kōan writes goes through `Config::persist()`, which sends secrets and machine paths to `config.local.toml` and removes the key from `config.toml` when it does. A secret in `config.toml` was put there by hand or by an older kōan; move it to `config.local.toml`. If kōan writes one there itself, report it as a bug.

## Logs

kōan logs to `~/.config/koan/koan.log`.

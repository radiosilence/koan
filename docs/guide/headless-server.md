# Headless Server

Run koan as a background music server with no TUI -- controlled entirely via the GraphQL API, Subsonic REST API, or MCP.

## Quick start

```bash
# Headless with GraphiQL IDE
koan --headless --playground

# Background daemon
koan -d

# Daemon with all APIs
koan -d --playground --subsonic 4040
```

## Daemon mode

The `-d` flag detaches koan from the terminal and runs it in the background:

```bash
koan -d
```

koan logs to `~/.config/koan/koan.log` in daemon mode. The GraphQL API is available on `http://localhost:4000/graphql` by default.

## Server flags

| Flag | Effect |
|------|--------|
| `--headless` | No TUI, API only |
| `--playground` | Enable GraphiQL web IDE at `GET /graphql` |
| `--subsonic PORT` | Enable Subsonic REST API on the given port |
| `--port PORT` | Custom GraphQL port (default: 4000) |
| `--bind ADDR` | Bind address (default: 127.0.0.1) |
| `-d` | Detach and run as background daemon |

## Configuration

```toml
[graphql]
enabled = true            # redundant in headless mode, but controls TUI+API mode
port = 4000               # GraphQL API port
bind = "127.0.0.1"        # bind address
playground = false         # GraphiQL IDE
subsonic_port = 4040       # Subsonic REST API port (default: disabled, set port to enable)
```

Or via environment variables:

```bash
export KOAN_GRAPHQL__PORT=8080
export KOAN_GRAPHQL__BIND=0.0.0.0
export KOAN_GRAPHQL__PLAYGROUND=true
```

## Docker

```bash
docker run -e KOAN_REMOTE__ENABLED=true \
           -e KOAN_REMOTE__URL=https://music.example.com \
           -e KOAN_REMOTE__USERNAME=admin \
           -e KOAN_REMOTE__PASSWORD="$NAVIDROME_PASSWORD" \
           -e KOAN_GRAPHQL__BIND=0.0.0.0 \
           -e KOAN_GRAPHQL__PORT=4000 \
           -p 4000:4000 \
           koan --headless
```

All config fields are overridable via `KOAN_*` environment variables. See [Configuration](../reference/configuration.md) for the full list.

## Remote TUI

Connect a TUI from another machine to a running headless koan:

```bash
koan --server http://host:4000          # full TUI
koan --server http://host:4000 --jukebox  # remote control only (no local playback)
```

## Security

The GraphQL API has **no authentication**. By default it binds to `127.0.0.1` (localhost only). If you expose it on `0.0.0.0`, anyone on your network can control playback, browse your library, move files (via organize), and clear your queue.

Only bind to all interfaces on trusted networks.

# Headless Server

Run kōan as a background music server with no TUI -- controlled entirely via the GraphQL API, Subsonic REST API, or MCP.

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

The `-d` flag detaches kōan from the terminal and runs it in the background:

```bash
koan -d
```

kōan logs to `~/.config/koan/koan.log` in daemon mode. The GraphQL API is available on `http://localhost:4000/graphql` by default.

## Server flags

| Flag | Effect |
|------|--------|
| `--headless` | No TUI, API only |
| `--playground` | Enable GraphiQL web IDE at `GET /graphql` |
| `--subsonic PORT` | Serve the Subsonic REST API on its own port as well |
| `--port PORT` | Custom GraphQL port (default: 4000) |
| `--bind ADDR` | Bind address (default: 127.0.0.1) |
| `--mcp-bind ADDR:PORT` | Also serve MCP over HTTP at `/mcp` (env `KOAN_MCP_BIND`). It trusts the `x-koan-username` / `x-koan-password` a gateway sends, so only that gateway may reach it; `KOAN_MCP_REQUIRE_LOGIN=1` refuses requests without them |
| `-d` | Detach and run as background daemon |

A headless server indexes the library folders when it starts and again whenever they change, as the macOS app does.

## Configuration

```toml
[graphql]
enabled = true            # redundant in headless mode, but controls TUI+API mode
port = 4000               # GraphQL API port
bind = "127.0.0.1"        # bind address
playground = false        # GraphiQL IDE

# config.local.toml — which machine serves Subsonic
[subsonic]
enabled = true            # mount /rest/* on the GraphQL port
port = 4040               # and on a dedicated port too (optional)
```

Or via environment variables:

```bash
export KOAN_GRAPHQL__PORT=8080
export KOAN_GRAPHQL__BIND=0.0.0.0
export KOAN_GRAPHQL__PLAYGROUND=true
```

## Remote TUI

Connect a TUI from another machine to a running headless kōan:

```bash
koan --server http://host:4000          # full TUI
koan --server http://host:4000 --jukebox  # remote control only (no local playback)
```

## Authentication

Auth is enabled by default. Run `koan auth setup` before starting the server to create a keypair and admin user. See [Authentication](authentication.md) for the full guide.

The API binds to `127.0.0.1` by default. If you expose it on `0.0.0.0` with `--bind 0.0.0.0`, make sure auth is enabled (it is by default) or restrict access at the network level.

Cookie auth needs `cookie_secure = false` over plain HTTP, and browser clients need their origin listed in `cors_origins`. If you reach the server through a hostname, add it to `allowed_hosts` — anything else is refused. See [Configuration](../reference/configuration.md#graphql).

To disable auth (localhost-only setups):

```toml
[graphql]
auth_enabled = false
```

> **Warning:** with auth disabled, anything that can reach the port is an admin — it can read your
> entire library, control playback and rewrite config. The `Origin` and `Host` checks keep a web page
> you visit from being that "anything", but they are not a substitute for auth: any other machine on
> the network still gets in. Only disable auth on a host you control, bound to `127.0.0.1`, and never
> with the port forwarded.

## Web UI

The server answers `http://host:4000/` with a browser UI: albums, artists, search, a play queue and share links, laid out for a phone as well as a desktop. Albums and artists sort and filter (favourites, lossless or codec, years, genre) through the page URL, so a filtered view can be bookmarked or sent. Playback happens in the browser, streaming from the server; the server's own player is not involved.

Sign in with a kōan account (`koan auth create-user`). The session is the same pair of `HttpOnly` cookies the JSON login sets, so behind plain HTTP the UI needs `cookie_secure = false`, and a hostname it is reached by must be in `allowed_hosts`. The access cookie lasts `access_token_ttl`; an open page renews it from the refresh cookie, and a page loaded after it lapsed renews on the way in. With `auth_enabled = false` the UI is open to anyone who can reach the port. Covers are resized once and kept in `covers/` in the config directory; deleting it only costs regenerating them.

## Sharing

A server makes share links itself: `createShare` (GraphQL, MCP, or a Subsonic client's own share button) returns `https://<public_url>/share/<id>`, a page anyone can open without an account, with a player for each shared track. Set where the server is reached from outside:

```toml
[sharing]
public_url = "https://koan.example.com"
```

A share is a slice of the library, shown the way the app shows it: one track shares its album cued to that track, an album the album, an artist their albums in release order, and several tracks stay a list. What a share covers is fixed when it is made; an artist share does not grow when the library does. Pages carry OpenGraph and Twitter card tags, with the cover as an absolute `og:image` on `public_url`, so a pasted link unfurls with its artwork.

The page and its audio answer for the share's own tracks and nothing else, addressed by position in the share rather than by library id. An expired, revoked or made-up id is the same 404. List shares with `shares`, set an expiry with `updateShare`, revoke with `deleteShare`. A kōan TUI or macOS app whose remote server is this kōan shares through it.

## In a container

The image at `ghcr.io/radiosilence/koan` runs `koan --headless --bind 0.0.0.0`, keeps config, database and auth keys in `/config`, and needs no sound card. Mount the library read-only, list it under `[library] folders` in `/config/config.toml`, and add the public hostname to `allowed_hosts`. Create the first user with `koan auth setup` inside the container, and `koan subsonic setup` to enable the Subsonic API.

MCP over HTTP (`KOAN_MCP_BIND=0.0.0.0:8081`) carries no credential of its own, like the stdio transport: put an authenticating gateway in front of it and let nothing else reach that port.

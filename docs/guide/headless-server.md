# Running a server

`koan --headless` runs kōan with no terminal UI, serving the library to other machines: a web UI, the Subsonic API, GraphQL and MCP, all on one port. The container image runs the same thing; see [In a container](#in-a-container).

## Quick start

```bash
koan auth setup                       # signing keys and the first admin account
koan subsonic setup                   # turn on the Subsonic API
koan --headless --bind 0.0.0.0        # serve on every interface, port 4000
```

## Server flags

| Flag | Effect |
|------|--------|
| `--headless` | No TUI, API only |
| `--playground` | Enable GraphiQL web IDE at `GET /graphql` |
| `--subsonic PORT` | Also serve the Subsonic API on a port of its own, for clients that expect one |
| `--port PORT` | Custom GraphQL port (default: 4000) |
| `--bind ADDR` | Bind address (default: 127.0.0.1) |
| `-d` | Detach and run in the background, logging to `~/.config/koan/koan.log` |

MCP is served at `/mcp` on the same port, with its own OAuth sign-in once `sharing.public_url` is set; see [MCP Integration](mcp-integration.md#connecting-to-a-server).

A headless server indexes the library folders when it starts and again whenever they change, as the macOS app does.

## Configuration

```toml
[graphql]
port = 4000               # the server's port
bind = "127.0.0.1"        # bind address
playground = false        # GraphiQL IDE

# config.local.toml — which machine serves Subsonic
[subsonic]
enabled = true            # serve /rest/* on the main port
port = 4040               # and on a port of its own (optional)
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
koan play --server http://host:4000            # full TUI
koan play --server http://host:4000 --jukebox  # remote control only (no local playback)
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

With auth disabled, anything that can reach the port is an admin; see [Recovery / lockout](authentication.md#recovery--lockout) before doing it.

## Web UI

The server answers `http://host:4000/` with a browser UI: albums, artists, playlists, search, a play queue and share links, laid out for a phone as well as a desktop. Albums and artists sort and filter (favourites, lossless or codec, years, genre) through the page URL, so a filtered view can be bookmarked or sent. Playlists are the account's own and anyone's public ones, played or queued like an album; they are edited from the apps. Playback happens in the browser, streaming from the server; the server's own player is not involved.

Sign in with a kōan account (`koan auth create-user`). The session is the same pair of `HttpOnly` cookies the JSON login sets, so behind plain HTTP the UI needs `cookie_secure = false`, and a hostname it is reached by must be in `allowed_hosts`. The access cookie lasts `access_token_ttl`; an open page renews it from the refresh cookie, and a page loaded after it lapsed renews on the way in. With `auth_enabled = false` the UI is open to anyone who can reach the port. Covers are resized once and kept in `covers/` in the config directory; deleting it only costs regenerating them.

## Sharing

A server makes share links itself: `createShare` (GraphQL, MCP, or a Subsonic client's own share button) returns `https://<public_url>/share/<id>`, a page anyone can open without an account, with a player for each shared track. Set where the server is reached from outside:

```toml
[sharing]
public_url = "https://koan.example.com"
```

A share is a slice of the library, shown the way the app shows it: one track shares its album cued to that track, an album the album, an artist their albums in release order, and several tracks stay a list. What a share covers is fixed when it is made; an artist share does not grow when the library does. Pages carry OpenGraph and Twitter card tags, with the cover as an absolute `og:image` on `public_url`, so a pasted link unfurls with its artwork.

The page and its audio answer for the share's own tracks and nothing else, addressed by position in the share rather than by library id. An expired, revoked or made-up id is the same 404. List shares with `shares`, set an expiry with `updateShare`, revoke with `deleteShare`. A kōan TUI or macOS app whose remote server is this kōan shares through it.

## Playing on a linked app

The macOS and iOS apps, signed in to a kōan server, hold a WebSocket open to it at `/rest/koanLink` (authenticated like any other `/rest` call; a kōan extension, not part of OpenSubsonic). Down it the server sends commands; up it each app reports what it is playing, where it is in the track, and its queue.

```graphql
{ clients { name platform playing nowPlaying positionMs queue { trackId title current } } }
mutation { playOnClient(trackIds: ["812", "813"]) { ok message } }
mutation { queueOnClientWhenAdded(artist: "Rilo Kiley", album: "Under the Blacklight") { id } }
```

| Mutation | Does |
|---|---|
| `playOnClient(trackIds, startAt, enqueue)` | Replace the queue and play, or append with `enqueue: true` |
| `playNextOnClient(trackIds)` | Insert after the current track |
| `jumpOnClient(trackId)` | Play that track: from the queue, or slotted in after the current one |
| `removeFromClient(trackIds)`, `clearClient` | Edit the queue |
| `controlClient(action)`, `seekOnClient(positionMs)` | Pause, resume, next, previous; seek |
| `syncClient` | Pull what the server has added since the app last synced |
| `queueOnClientWhenAdded(artist, album, playNext)` | Queue an album once it is in the library, e.g. one slsk is downloading. Checked after every library scan; lapses after a day. `clientOrders` lists them, `cancelClientOrder` withdraws one |

Every mutation takes an optional `client`, an id or name from `clients`. Without it the server picks the app that is playing, else the one that played in the last six hours, else the only one linked, and otherwise answers with the choices so the caller can ask. Track ids are the server's; an app that has not seen one syncs first, and its pages show what the sync brought in. A user sees and commands their own account's apps; an admin sees everyone's.

An app is linked while it runs. iOS suspends a backgrounded app that is not playing, and the server drops a link it has not heard from in 100 seconds. The iOS app's **Stay reachable when paused** setting (Settings → Playback) keeps it running after a pause by playing silence: indefinitely on the charger, and for a chosen time on battery, since it keeps the audio hardware awake. Opening the app relinks at once.

## In a container

The image at `ghcr.io/radiosilence/koan` runs `koan --headless --bind 0.0.0.0`, keeps config, database and auth keys in `/config`, and needs no sound card. `latest` and `vX.Y.Z` are releases; `main` and a commit sha follow the main branch between them. Mount the library read-only, list it under `[library] folders` in `/config/config.toml`, and add the public hostname to `allowed_hosts`. Create the first user with `koan auth setup` inside the container, and `koan subsonic setup` to enable the Subsonic API.

Set `sharing.public_url` to the public address (`KOAN_SHARING__PUBLIC_URL`) for share links and for MCP clients to sign in at `/mcp`.

### Docker Compose

[`deploy/compose/compose.yaml`](https://github.com/radiosilence/koan/blob/main/deploy/compose/compose.yaml) runs kōan behind Caddy, which obtains and renews the TLS certificate. On a machine whose hostname resolves to it, with ports 80 and 443 open:

```bash
curl -O https://raw.githubusercontent.com/radiosilence/koan/main/deploy/compose/compose.yaml
KOAN_HOST=music.example.com MUSIC=/mnt/music docker compose up -d
docker compose exec koan koan auth setup   # the admin account
```

Then open `https://music.example.com` and sign in. The Subsonic API is on for kōan accounts; `koan subsonic setup` adds a shared secret for clients that have none. Config, the database and keys live in the `koan-config` volume. The image runs as uid 1000, so a bind mount in its place has to be writable by that uid.

On Kubernetes, a versioned Pulumi component package, [`@radiosilence/koan-pulumi`](https://github.com/radiosilence/koan/pkgs/npm/koan-pulumi), is published to GitHub Packages alongside each release. It exports `createKoan`, which builds the Deployment, its init container, Services and NetworkPolicy from a config object validated against `KoanConfSchema`.

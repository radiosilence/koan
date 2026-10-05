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

Control a running headless kōan from a TUI on another machine. The server plays
the audio; the TUI is a remote control:

```bash
koan play --server http://host:4000
```

To play its library on the machine you are sitting at, sign in to it as a remote
server instead (`koan remote login`, or Settings in the apps). Tracks then play
through the local engine and download into the local cache like any other remote
library.

## Authentication

Auth is enabled by default. See [Authentication](authentication.md) for the full guide.

### The first account

A new server has no accounts, and nothing can sign in until one exists. There is no sign-up page: on a server reachable from the internet, whoever found it first would become its admin. The first admin is made with `koan auth setup`, run on the server itself, against the same config directory the server uses. The server can be running at the time; it picks the account up on the next sign-in.

| Where the server runs | Run |
|---|---|
| Directly on a machine | `koan auth setup` |
| `docker run` | `docker exec -it koan koan auth setup` |
| Docker Compose | `docker compose exec koan koan auth setup` |
| Kubernetes | `kubectl -n koan exec -it deploy/koan -- koan auth setup` |

It asks for a username and offers to generate a password, which it prints once. To run it without a terminal, from a script or a CI job, pass both in the environment:

```bash
docker exec -e KOAN_USERNAME=admin -e KOAN_PASSWORD='…' koan koan auth setup
```

Once there is an admin, everything else is done signed in: the web UI's Users page, the apps' Settings, or GraphQL create accounts and send [invites](authentication.md#invites), so nobody else needs a shell. `koan auth setup` does nothing on a server that already has accounts; `koan auth create-user` adds one from the shell, and `koan auth reset-password` recovers an admin who is locked out.

The API binds to `127.0.0.1` by default. If you expose it on `0.0.0.0` with `--bind 0.0.0.0`, make sure auth is enabled (it is by default) or restrict access at the network level.

Cookie auth needs `cookie_secure = false` over plain HTTP, and browser clients need their origin listed in `cors_origins`. If you reach the server through a hostname, add it to `allowed_hosts` — anything else is refused. See [Configuration](../reference/configuration.md#graphql).

To disable auth (localhost-only setups):

```toml
[graphql]
auth_enabled = false
```

With auth disabled, anything that can reach the port is an admin; see [Recovery / lockout](authentication.md#recovery--lockout) before doing it.

## Web UI

The server answers `http://host:4000/` with a browser UI: albums, artists, playlists, search, a play queue and share links, laid out for a phone as well as a desktop. Albums and artists sort and filter (name, favourites, lossless or codec, years, genre) through the page URL, the same filters the macOS and iOS apps offer, so a filtered view can be bookmarked or sent. Playlists are the account's own and anyone's public ones, played or queued like an album; they are edited from the apps. Favourites lists the signed-in account's favourite artists, records and tracks, as the apps' page does; Recently played what it played in the last 30 days, each once; and History its plays by day, any of which can be ticked and forgotten. On a phone these sit under a Library tab with Playlists, since the tab bar has room for six. Playback happens in the browser, streaming from the server; the server's own player is not involved.

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

Every mutation takes an optional `client`, an id or name from `clients`. Without it the server picks the app that is playing, else the one that played in the last six hours, else the only one linked, and otherwise answers with the choices so the caller can ask. Track ids are the server's; an app that has not seen one syncs first, and its pages show what the sync brought in. Every account, admin included, sees and commands only its own apps, so a server shared by several people never sends one person's music to another's phone.

An app is linked while it runs. iOS suspends a backgrounded app that is not playing, and the server drops a link it has not heard from in 100 seconds. A server with [`[push]`](../reference/configuration.md#push) configured wakes a suspended phone to relink and take the command, choosing the one seen most recently when no `client` is named, since a reinstalled app leaves its old entry behind under the same name; a request to play arrives as a notification to tap, since iOS does not let an app start audio from the background. Opening the app relinks at once.

## In a container

The image at `ghcr.io/radiosilence/koan` runs `koan --headless --bind 0.0.0.0`, keeps config, database and auth keys in `/config`, and needs no sound card. `latest` and `vX.Y.Z` are releases; `main` and a commit sha follow the main branch between them. Mount the library read-only, list it under `[library] folders` in `/config/config.toml`, and add the public hostname to `allowed_hosts`. Create the first admin with `koan auth setup` inside the container (see [The first account](#the-first-account)), and `koan subsonic setup` to enable the Subsonic API.

Set `sharing.public_url` to the public address (`KOAN_SHARING__PUBLIC_URL`) for share links and for MCP clients to sign in at `/mcp`.

### Kubernetes

The server needs one pod with two volumes: the library, read-only, and a state directory at `/config` that outlives the pod. It is a single SQLite index, so it runs as one replica and is replaced rather than rolled. Set `KOAN_LIBRARY__FOLDERS`, `KOAN_GRAPHQL__ALLOWED_HOSTS` and `KOAN_SHARING__PUBLIC_URL` as in the Compose example below, and terminate TLS in front of it.

#### With Pulumi

[`@radiosilence/koan-pulumi`](https://github.com/radiosilence/koan/pkgs/npm/koan-pulumi) is a Pulumi component that deploys the server into a namespace. Its version is koan's: each release publishes both, and a pinned package deploys exactly that image.

It is published to GitHub Packages, which wants a token with `read:packages` even for public packages:

```ini
# .npmrc
@radiosilence:registry=https://npm.pkg.github.com
//npm.pkg.github.com/:_authToken=${GITHUB_TOKEN}
```

```bash
npm install @radiosilence/koan-pulumi @pulumi/kubernetes @pulumi/pulumi zod
```

`createKoan(provider, namespace, config)` validates `config` against `KoanConfSchema` and creates a Deployment, a Service on port 80 and a NetworkPolicy. It creates no Ingress; it returns `routes` naming the Service and hostname, for whatever fronts the cluster to route.

```ts
import * as k8s from "@pulumi/kubernetes";
import { createKoan } from "@radiosilence/koan-pulumi";

const provider = new k8s.Provider("cluster", {});
const ns = new k8s.core.v1.Namespace("koan", { metadata: { name: "koan" } }, { provider });

const koan = createKoan(provider, ns.metadata.name, {
  hostname: "music.example.com",
  library: { existingClaim: "music" },
  state: { existingClaim: "koan-state" },
});

export const routes = koan.routes;
```

| Option | |
|--------|-|
| `hostname` | Required. Becomes `allowed_hosts` and `sharing.public_url` (`https://<hostname>`), so TLS is expected in front |
| `library` | Required: `existingClaim` or `hostPath`. Mounted read-only at `/music` |
| `state` | `existingClaim` or `hostPath`, mounted at `/config`. Unset, it is an `emptyDir` and the index and accounts go with the pod |
| `image` | `repository`, `tag` (defaults to the package's version), `pullPolicy` |
| `nodeSelector` | Needed with a `hostPath`, which is one node's disk |
| `push` | `existingSecret` holding the APNs key under `apns-key`, plus `keyId` and `teamId`: wakes the iOS app when it is suspended |
| `resources` | Requests default to 250m / 256Mi, limits to 6 CPU / 5Gi |
| `networkPolicy` | On by default. Ingress only from `api.from` (a Traefik pod by default) plus `extraIngress`; egress to cluster DNS and the public internet, with `privateCidrs` excluded |

With persistent state, each update first runs `koan check-db` in a Job against a snapshot of the live database. If the new version's migration fails there, the update stops and the old pod keeps serving. kubelet creates a missing `hostPath` as root, and koan runs as uid 1000, so an init container hands the state directory to that uid before the server starts; `initPermissions.enabled: false` turns it off. The root filesystem is read-only, and the artwork and lyrics caches live in an `emptyDir`, rebuilt after a restart.

Unknown options are rejected rather than ignored, so a stack carrying options a newer package removed fails at `pulumi preview`.

Once it is running, create the admin account in the pod:

```bash
kubectl -n koan exec -it deploy/koan -- koan auth setup
```

### Docker Compose

kōan behind Caddy, which obtains and renews the TLS certificate. Save as `compose.yaml`:

```yaml
services:
  koan:
    image: ghcr.io/radiosilence/koan:latest
    restart: unless-stopped
    environment:
      KOAN_LIBRARY__FOLDERS: '["/music"]'
      KOAN_GRAPHQL__ALLOWED_HOSTS: '["${KOAN_HOST:?set KOAN_HOST to the server hostname}"]'
      KOAN_GRAPHQL__COOKIE_SECURE: "true"
      KOAN_SHARING__PUBLIC_URL: https://${KOAN_HOST}
      KOAN_SUBSONIC__ENABLED: "true"
    volumes:
      # The image runs as uid 1000; a bind mount here has to be writable by it.
      - koan-config:/config
      - ${MUSIC:?set MUSIC to the music folder}:/music:ro

  caddy:
    image: caddy:2
    restart: unless-stopped
    ports: ["80:80", "443:443", "443:443/udp"]
    command: caddy reverse-proxy --from ${KOAN_HOST} --to koan:4000
    volumes:
      - caddy-data:/data

volumes:
  koan-config:
  caddy-data:
```

On a machine whose hostname resolves to it, with ports 80 and 443 open:

```bash
KOAN_HOST=music.example.com MUSIC=/mnt/music docker compose up -d
docker compose exec koan koan auth setup   # the admin account
```

Then open `https://music.example.com` and sign in. The Subsonic API is on for kōan accounts; `koan subsonic setup` adds a shared secret for clients that have none. Config, the database and keys live in the `koan-config` volume. The image runs as uid 1000, so a bind mount in its place has to be writable by that uid.

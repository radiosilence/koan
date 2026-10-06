# Migrating from Navidrome

A kōan server does the job Navidrome does: it indexes a music folder, serves it over the Subsonic API with OpenSubsonic extensions, and gives each person an account. Subsonic apps that work with Navidrome work with kōan.

You do not have to switch to use kōan's apps. The macOS and iOS apps and the terminal UI play from a Navidrome server as they do from a kōan one, so a move can be gradual: run kōan beside Navidrome on the same read-only music folder, try it, and move clients one at a time.

## What you gain

- **Apps that control each other.** kōan's apps keep a connection open to a kōan server. Any device can see what another is playing, take over its queue, or hand the music to it, across networks. Against Navidrome they can only find each other on the local network. See [Playing on another device](devices.md).
- **Changes arrive without polling.** A kōan server tells linked apps when the library or a playlist changes. Against Navidrome, apps check on a timer.
- **An assistant can drive it.** The server is an MCP server with its own OAuth sign-in. Claude and other assistants connect as a kōan account and can search the library, build playlists and play music on that account's devices. See [MCP integration](mcp-integration.md).
- **Native players.** The macOS app and terminal UI play bit-perfect where the device allows, gapless, and cache what they stream so it plays again offline. The server itself needs no sound card.

## What you give up

These Navidrome features have no kōan equivalent today:

- **Scrobbling to Last.fm** ([#773](https://github.com/radiosilence/koan/issues/773)). kōan forwards plays to ListenBrainz (see [Scrobbling](headless-server.md#scrobbling)), not to Last.fm.
- **Serving under a sub-path** (`ND_BASEURL`, [#760](https://github.com/radiosilence/koan/issues/760)). kōan expects its own hostname.
- **Reverse-proxy authentication** (`ND_REVERSEPROXYUSERHEADER`, [#769](https://github.com/radiosilence/koan/issues/769)). Accounts are kōan's own.
- **Per-library permissions** ([#762](https://github.com/radiosilence/koan/issues/762)). Every account sees the whole library.
- **Internet radio**, which is not planned. Clients that use it lose the feature against kōan; the rest of the client works.

## Translating a Navidrome setup

### The container

A typical Navidrome compose file:

```yaml
services:
  navidrome:
    image: deluan/navidrome:latest
    ports: ["4533:4533"]
    environment:
      ND_SCANSCHEDULE: 1h
      ND_ENABLESHARING: "true"
    volumes:
      - ./data:/data
      - /mnt/music:/music:ro
```

The kōan equivalent:

```yaml
services:
  koan:
    image: ghcr.io/radiosilence/koan:latest
    ports: ["4000:4000"]
    environment:
      KOAN_LIBRARY__FOLDERS: '["/music"]'
      KOAN_GRAPHQL__ALLOWED_HOSTS: '["music.example.com"]'
      KOAN_SHARING__PUBLIC_URL: https://music.example.com
      KOAN_SUBSONIC__ENABLED: "true"
    volumes:
      - koan-config:/config
      - /mnt/music:/music:ro
```

with `koan-config` declared under `volumes:`. Navidrome makes its first admin on the sign-in page; kōan has no sign-up page, so the first admin is created from a shell in the container (see [The first account](headless-server.md#the-first-account)):

```bash
docker compose exec koan koan auth setup
```

[Docker Compose](headless-server.md#docker-compose) has a complete file with Caddy in front for TLS.

| Navidrome | kōan |
|-----------|------|
| `/data` | `/config`: config, database, signing keys and the cover cache. The image runs as uid 1000, so a bind mount must be writable by it |
| `ND_MUSICFOLDER` | `KOAN_LIBRARY__FOLDERS`, a list; several folders form one library |
| `ND_SCANSCHEDULE`, `ND_SCANNER_WATCHERWAIT` | Nothing to set. The server scans at start and watches the folders for changes |
| `ND_ENABLESHARING` | Always on; `sharing.public_url` sets the address links are built on |
| `ND_BASEURL` | Not supported; give kōan its own hostname |
| Port 4533 | Port 4000, which serves the web UI, the Subsonic API (`/rest`), GraphQL and MCP |

kōan only reads the music folder, as Navidrome does. Both can mount the same folder at once while you try kōan out.

### The reverse proxy

Point the proxy at port 4000 instead of 4533, and list the hostname in `allowed_hosts`: kōan refuses requests for any `Host` it was not given. The linked apps hold a WebSocket open at `/rest/koanLink`, so the proxy must pass WebSocket upgrades. Caddy does this by default:

```
music.example.com {
    reverse_proxy koan:4000
}
```

### Accounts

Navidrome's accounts do not carry over. With the admin account made, create the rest on the web UI's Users page, or from the shell:

```bash
koan auth create-user --username alice --role user
koan auth invite alice --server https://music.example.com
```

The invite is a link that signs kōan's apps in directly and shows the details for any other Subsonic app. The web UI's Users page does the same. Roles are `admin`, `user` and `readonly`; Navidrome's admin and regular users map to the first two.

### Subsonic clients

Change the server URL and sign in with the kōan account. kōan accepts every way Subsonic clients sign in:

- **Password**, plain or hex-encoded.
- **Token and salt**, which most clients use by default, with an app password in place of the account's password.
- **API key** (OpenSubsonic `apiKeyAuthentication`).

Make API keys and app passwords on the web UI's Account page, one per client. A client that reports error 41 is using token auth with the account's own password, which kōan cannot check because it keeps only a hash: give it an app password instead. See [Authentication](authentication.md#subsonic-api).

### Favourites, play counts and playlists

There is no importer for Navidrome's database yet; [#651](https://github.com/radiosilence/koan/issues/651) tracks one. Until then, favourites, ratings, play counts and the playlists made in Navidrome stay there and start empty in kōan.

Covers kept as image files beside the tracks carry over. kōan looks for `cover.*`, `folder.*` and `front.*` (JPEG, PNG or WebP, any case) in that order, then for art embedded in the files, which is Navidrome's default `CoverArtPriority`. A different `CoverArtPriority` is not read; kōan always uses this order.

Playlists kept as files in the music folder carry over: kōan reads Navidrome's smart playlists (`.nsp`) and `.m3u`/`.m3u8` files when it scans. See [Smart playlists](smart-playlists.md#from-navidrome).

## Running both

To keep Navidrome running while moving across:

1. Run kōan beside it on the same read-only music mount, on its own hostname.
2. Sign kōan's apps in to the kōan server, and leave other clients on Navidrome.
3. Move the remaining clients once kōan covers what they use, then stop Navidrome.

kōan's apps can stay pointed at Navidrome throughout. They sync its library, favourites and playlists, so nothing about them has to change until the server does.

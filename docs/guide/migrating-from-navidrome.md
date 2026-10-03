# Migrating from Navidrome

A kōan server does the job Navidrome does: it indexes a music folder, serves it over the Subsonic API with OpenSubsonic extensions, and gives each person an account. Subsonic apps that work with Navidrome work with kōan. This page covers what changes, how a typical Navidrome setup translates, and what kōan does not do.

You do not have to switch to use kōan's apps. The macOS and iOS apps and the terminal UI play from a Navidrome server as they do from a kōan one, so a move can be gradual: run kōan beside Navidrome on the same read-only music folder, try it, and move clients one at a time.

## What you gain

- **Apps that control each other.** kōan's apps keep a connection open to a kōan server. Any device can see what another is playing, take over its queue, or hand the music to it, across networks. Against Navidrome they can only find each other on the local network. See [Playing on another device](devices.md).
- **Changes arrive without polling.** A kōan server tells linked apps when the library or a playlist changes. Against Navidrome, apps check on a timer.
- **An assistant can drive it.** The server is an MCP server with its own OAuth sign-in. Claude and other assistants connect as a kōan account and can search the library, build playlists and play music on that account's devices. See [MCP integration](mcp-integration.md).
- **Share pages that play.** A share link opens a page with a player that works without an account, and unfurls with its cover when pasted.
- **Native players.** The macOS app and terminal UI play bit-perfect where the device allows, gapless, and cache what they stream so it plays again offline. The server itself needs no sound card.

## What you give up

These Navidrome features have no kōan equivalent today:

- **Transcoding.** kōan streams the original file. A client that asks for a lower bitrate gets the original, so lossless libraries cost full bandwidth on mobile data. kōan's own apps cache what they play, which limits the cost to the first play.
- **Scrobbling to Last.fm or ListenBrainz.** kōan records plays in its own history, per account, and does not forward them.
- **Ratings.** Stars (favourites) are supported; one-to-five ratings are not.
- **Smart playlists** (`.nsp`) and **importing `.m3u` files** from the music folder.
- **Serving under a sub-path** (`ND_BASEURL`). kōan expects its own hostname.
- **Reverse-proxy authentication** (`ND_REVERSEPROXYUSERHEADER`). Accounts are kōan's own.
- **Per-library permissions.** Every account sees the whole library.
- **Internet radio, bookmarks and server-side play queues** (`getPlayQueue`, `savePlayQueue`). Clients that use these lose the feature against kōan; the rest of the client works.

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

with `koan-config` declared under `volumes:`. Then create the admin account:

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

Navidrome's accounts do not carry over. Create each one in kōan:

```bash
koan auth create-user --username alice --role user
koan auth invite alice --server https://music.example.com
```

The invite is a link that signs kōan's apps in directly and shows the details for any other Subsonic app. The web UI's Users page does the same. Roles are `admin`, `user` and `readonly`; Navidrome's admin and regular users map to the first two.

### Subsonic clients

Change the server URL and sign in with the kōan account. kōan accepts every way Subsonic clients sign in:

- **Password**, plain or hex-encoded.
- **Token and salt**, which most clients use by default.
- **API key** (OpenSubsonic `apiKeyAuthentication`). Each person can make and revoke keys on the web UI's API keys page, one per client.

A client that reports error 41 is using token auth on an account created before kōan supported it; sign in once with the password, or reset it, and token auth works from then on. See [Authentication](authentication.md#subsonic-api).

### Favourites, play counts and playlists

There is no importer yet; [#651](https://github.com/radiosilence/koan/issues/651) tracks one. Until then, favourites, play counts and playlists stay in Navidrome and start empty in kōan.

## Running both

To keep Navidrome running while moving across:

1. Run kōan beside it on the same read-only music mount, on its own hostname.
2. Sign kōan's apps in to the kōan server, and leave other clients on Navidrome.
3. Move the remaining clients once kōan covers what they use, then stop Navidrome.

kōan's apps can stay pointed at Navidrome throughout. They sync its library, favourites and playlists, so nothing about them has to change until the server does.

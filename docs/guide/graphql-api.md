# GraphQL API

The GraphQL API runs alongside the TUI by default (port 4000, localhost only), or standalone in headless mode.

## Quick start

```bash
# TUI + API (default)
koan

# Headless with GraphiQL web IDE
koan --headless --playground

# As a background daemon
koan -d --playground
```

Then open `http://localhost:4000/graphql` for the GraphiQL IDE, or query directly:

```bash
# If auth is enabled (default), get a token first
TOKEN=$(curl -s -X POST http://localhost:4000/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username": "admin", "password": "your-password"}' | jq -r '.access_token')

curl -s http://localhost:4000/graphql \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"query": "{ nowPlaying { state, track { title, artist } } }"}'
```

## Authentication

Auth is enabled by default. Run `koan auth setup` to create a keypair and first admin user before starting the server. See [Authentication](authentication.md) for the full guide.

```bash
koan auth setup              # generate keypair + create admin user
koan --headless --playground # start server
```

To disable auth:

```toml
[graphql]
auth_enabled = false
```

With auth disabled, anything that can reach the port is an admin; see [Recovery / lockout](authentication.md#recovery--lockout) before doing it.

## Configuration

```toml
[graphql]
enabled = true                # run alongside TUI (default: true, --no-api disables)
port = 4000                   # API port (default: 4000)
bind = "127.0.0.1"            # bind address (default: 127.0.0.1)
playground = false             # GraphiQL IDE at GET /graphql (default: false)
auth_enabled = true           # JWT authentication (default: true)
access_token_ttl = "15m"      # access token lifetime (default: 15m)
refresh_token_ttl = "30d"     # refresh token lifetime (default: 30d)
```

The server binds to `127.0.0.1` by default. Use `--bind 0.0.0.0` or `bind = "0.0.0.0"` in config to expose on all interfaces.

## Example queries

### Library browsing

```graphql
# Find early FLAC albums
{
  albums(yearEnd: 1995, codec: "FLAC") {
    edges { node { title, artistName, date } }
  }
}

# Hi-res techno tracks
{
  tracks(genre: "techno", minSampleRate: 96000, minBitDepth: 24) {
    edges { node { title, artist, codec, sampleRate } }
  }
}

# Nested: artist -> albums -> tracks
{
  artists(search: "Aphex") {
    edges {
      node {
        name
        albums {
          edges {
            node {
              title
              tracks { edges { node { title } } }
            }
          }
        }
      }
    }
  }
}
```

### Playback control

```graphql
# What's playing?
#
# `seekableMs` is how far in a seek can land. It equals `durationMs` for
# anything on disk and falls short of it while a track is still downloading,
# so a client draws its seek bar against it rather than letting someone scrub
# into audio that has not arrived.
{
  nowPlaying {
    state
    positionMs
    durationMs
    seekableMs
    shuffle
    repeat
    track { title, artist, codec, sampleRate, bitDepth }
  }
}

# Shuffle reorders the rest of the queue itself; off puts it back in its
# original order. Repeat is OFF, QUEUE or ONE. `setPlayModeOnClient` does the
# same on a linked app, whose modes `clients { shuffle repeat }` reports.
mutation { setPlayMode(shuffle: true, repeat: QUEUE) { ok } }

# Queue management
mutation { replaceQueue(trackIds: [42, 43, 44]) { ok, addedCount } }
mutation { saveQueueAsPlaylist(name: "techno friday") { id, name, trackCount } }

# Playlists
{ playlists { id, name, trackCount, durationMs, remoteId } }
{ playlistTracks(id: 3) { id, title, artist } }
mutation { addToPlaylist(id: 3, trackIds: [42, 43]) { ok } }
# A reorder, a removal and a shuffle are all the same call: the list you want.
mutation { setPlaylistTracks(id: 3, trackIds: [43, 42]) { ok } }
mutation { playPlaylist(id: 3, shuffled: true) { ok } }
```

### Filtering

Collection queries filter on:

- **Albums**: year range, codec, label, genre
- **Tracks**: genre, codec, sample rate, bit depth, duration
- **Artists**: genre

All string filters are case-insensitive substrings. `%` and `_` in a filter value are literal, not
wildcards.

### Pagination and sorting

Collections are Relay connections (`edges` + `pageInfo`). **A collection with no `first` returns 50
rows, and `first` is capped at 500** — the API will not hand back a whole library in one response.
Page with the cursor from `pageInfo.endCursor`:

```graphql
{
  tracks(first: 200, after: "199", sortBy: TITLE, sortDir: DESC) {
    edges { cursor node { title } }
    pageInfo { hasNextPage endCursor }
  }
}
```

`sortBy` accepts `TITLE`, `ARTIST`, `ALBUM`, `DURATION` or `ARTIST_ALBUM_DISC_TRACK` for tracks;
`NAME`, `ALBUM_COUNT` or `TRACK_COUNT` for artists; `TITLE`, `DATE`, `ARTIST_THEN_DATE` or
`TRACK_COUNT` for albums.

### Long-running work

`triggerScan` and `triggerRemoteSync` run for minutes, so they return a job handle immediately and
do the work on a detached thread:

```graphql
mutation { triggerScan { id kind state } }
{ job(id: "0199...") { state message } }
```

`state` is `RUNNING`, `SUCCEEDED` or `FAILED`; `message` carries the outcome. Only one job of each
kind runs at a time — calling again while one is live returns the running job rather than starting a
second.

### Limits

Requests are refused rather than queued when the server is saturated, so a slow client cannot make
every other client slow:

- query depth 12, complexity 2000
- 30 second timeout on a query (`408`); subscriptions are exempt
- 64 queries in flight, beyond which further requests get `503` immediately

## The schema

`--playground` serves GraphiQL at `/graphql`, which browses the schema. The MCP tool `schema_sdl` returns the same schema as SDL.

## Subsonic API

The server's Subsonic API, at `/rest` on the same port, is configured separately; see [Configuration](../reference/configuration.md#subsonic) and [Authentication](authentication.md#subsonic-api).

## MCP server

The MCP server (`koan mcp`) uses the same GraphQL schema in-process (no HTTP round-trip). See [MCP Integration](mcp-integration.md) for Claude Desktop setup.

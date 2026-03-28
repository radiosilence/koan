# GraphQL API

koan exposes a GraphQL API for full programmatic control. The API runs alongside the TUI by default (port 4000, localhost only), or standalone in headless mode.

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
curl -s http://localhost:4000/graphql \
  -H 'Content-Type: application/json' \
  -d '{"query": "{ nowPlaying { state, track { title, artist } } }"}'
```

## Configuration

```toml
[graphql]
enabled = true                # run alongside TUI (default: true, --no-api disables)
port = 4000                   # API port (default: 4000)
bind = "127.0.0.1"            # bind address (default: 127.0.0.1)
playground = false             # GraphiQL IDE at GET /graphql (default: false)
subsonic_port = 4040           # optional Subsonic REST API port (default: disabled)
```

The server binds to `127.0.0.1` by default. Use `--bind 0.0.0.0` or `bind = "0.0.0.0"` in config to expose on all interfaces. There's no authentication, so only do this on trusted networks.

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
{
  nowPlaying {
    state
    positionMs
    track { title, artist, codec, sampleRate, bitDepth }
  }
}

# Queue management
mutation { replaceQueue(trackIds: [42, 43, 44]) { ok, addedCount } }
mutation { saveSnapshot(name: "techno friday") { ok } }
mutation { enableRadio { ok } }
```

### Filtering

Every query supports rich filtering:

- **Albums**: year range, codec, label, genre
- **Tracks**: genre, codec, sample rate, bit depth, duration
- **Artists**: genre

All string filters are case-insensitive substrings. Relay-style cursor pagination is available on all collection queries.

## Available operations

### Queries

| Category | Operations |
|----------|-----------|
| **Library** | `artists`, `albums`, `tracks`, `track`, `randomTracks`, `fuzzySearch`, `libraryStats` |
| **Playback** | `nowPlaying`, `queue`, `devices`, `lyrics`, `coverArt` |
| **Favourites** | `favourites` |
| **Snapshots** | `snapshots` |
| **Radio** | `radioStatus`, `similarTracks`, `similarArtists` |
| **History** | `playHistory` |

### Mutations

| Category | Operations |
|----------|-----------|
| **Playback** | `play`, `pause`, `resume`, `stop`, `next`, `previous`, `seek` |
| **Queue** | `addToQueue`, `replaceQueue`, `removeFromQueue`, `moveInQueue`, `clearQueue` |
| **Device** | `setDevice`, `clearDevice` |
| **Favourites** | `favourite`, `unfavourite`, `toggleFavourite` |
| **Snapshots** | `saveSnapshot`, `restoreSnapshot`, `deleteSnapshot` |
| **Radio** | `enableRadio`, `disableRadio` |
| **Organize** | `organizePreview`, `organizeExecute`, `organizeUndo` |
| **Library** | `triggerScan`, `triggerRemoteSync` |
| **Sharing** | `createShare` |
| **Undo** | `undo`, `redo` |

## Subsonic REST API

koan can also expose a Subsonic-compatible REST API for clients that speak the Subsonic protocol:

```bash
koan --headless --subsonic 4040
```

This runs on a separate port from the GraphQL API. Useful for connecting Subsonic clients (DSub, Ultrasonic, play:Sub) to a headless koan instance.

## MCP server

The MCP server (`koan mcp`) uses the same GraphQL schema in-process (no HTTP round-trip). See [MCP Integration](mcp-integration.md) for Claude Desktop setup.

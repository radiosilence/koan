# Smart playlists

A smart playlist holds whatever matches its rules: the tracks played most this year, favourites not heard in a month, every 24-bit jazz record added since spring. kōan evaluates the rules again when the playlist is read, at most once a minute, so it follows the library and your listening without anyone editing it.

To every client it is an ordinary playlist that cannot be edited. Subsonic apps see it marked read-only (OpenSubsonic's `readonly`), kōan's apps refuse drops, removals and reorders on it, and it is not offered under Add to Playlist. Change what it holds by changing its rules.

## Making one

From an assistant connected over [MCP](mcp-integration.md), ask for it: "make a playlist of my most played tracks that I haven't heard in a month". The assistant writes the rules.

Over [GraphQL](graphql-api.md):

```graphql
mutation {
  createSmartPlaylist(
    name: "Forgotten favourites"
    rules: {
      rules: [
        { field: "favourite", op: "is", value: true }
        { field: "lastPlayed", op: "notInTheLast", value: 30 }
      ]
      sort: [{ field: "playCount", desc: true }]
      limit: 50
    }
  ) { id trackCount }
}
```

`setPlaylistRules(id, rules)` changes the rules of an existing playlist. Given to an ordinary playlist, it becomes a smart one; `rules: null` makes a smart playlist ordinary again, keeping what it holds.

From a file: put a Navidrome smart playlist (`.nsp`) anywhere in a library folder and the next scan reads it. The folder watcher notices new and changed files. The file stays authoritative: editing it changes the playlist, deleting it deletes the playlist.

## Rules

A rule set has `match` (`all`, the default, or `any`), a list of `rules`, an optional `sort` and an optional `limit`. Each entry in `rules` is a condition, `{ field, op, value }`, or a nested group with its own `match` and `rules`.

| Fields | Operators | Value |
|--------|-----------|-------|
| `title`, `artist`, `albumArtist`, `album`, `genre`, `format`, `path` | `is`, `isNot`, `contains`, `notContains`, `startsWith`, `endsWith` | Text; case and spacing are ignored |
| `year`, `duration` (seconds), `bitDepth`, `sampleRate`, `trackNumber`, `discNumber`, `playCount` | `is`, `isNot`, `gt`, `lt`, `inTheRange` | A number, or `[low, high]` for a range |
| `lastPlayed`, `dateAdded` | `before`, `after`, `inTheRange`, `inTheLast`, `notInTheLast` | `"YYYY-MM-DD"`, two of them for a range, or a number of days |
| `favourite` | `is`, `isNot` | `true` or `false` |

`dateAdded` is when the track's album entered the library. A track never played counts as not played in the last any number of days.

`sort` is a list of `{ field, desc }`, applied in order; any field above can sort, and `random` shuffles. Without one, tracks come in album order. A random order is drawn once a day rather than on every read, so the playlist does not change each time an app syncs.

## Whose plays

`playCount`, `lastPlayed` and `favourite` are the owner's: the account that made the playlist. Playlists read from `.nsp` files belong to the first admin and are private, as Navidrome imports them.

## From Navidrome

kōan reads `.nsp` files with the fields above under Navidrome's names (`loved` for `favourite`, `filetype` for `format`, `filepath` for `path`), its operators, and `sort`/`order`/`limit`, including comma-separated sort keys with a `-` for descending. A file that uses something kōan has no counterpart for (ratings, BPM, comments, other playlists) is skipped and the reason logged: a playlist that ignored one of its conditions would hold more than its author asked for.

## M3U files

`.m3u` and `.m3u8` files in a library folder are read the same way, into ordinary playlists of the tracks they list. Entries may be relative to the file, absolute, or `file://` URLs; Windows separators are understood, and a plain `.m3u` that is not UTF-8 is read as Latin-1. `#PLAYLIST:` names the playlist, otherwise the file does. Entries that are not in the library, stream URLs among them, are left out and counted in the log, and a file that names nothing in the library is not imported.

As with `.nsp` files, the file decides: changing it changes the playlist at the next scan, deleting it deletes the playlist, and the playlist takes no edits in the meantime. To edit one in kōan, make a copy of it.

A playlist that a Subsonic server marks read-only, as OpenSubsonic servers mark their smart playlists, is shown read-only in kōan's apps and never pushed back to the server.

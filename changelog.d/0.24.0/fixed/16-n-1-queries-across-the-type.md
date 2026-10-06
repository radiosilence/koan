- **N+1 queries across the type graph.** `Track.isFavourite` opened a connection and scanned the
  whole `favourites` table per track; `Album.trackCount` and `totalDurationMs` each materialised
  every row of the album to count or sum them; `Artist.albumCount`/`trackCount` re-ran the query
  their sibling field had just run. Counts and sums are now `COUNT(*)`/`SUM(...)` in SQLite, and
  parent → child edges go through dataloaders, so `{ tracks(first: 500) { isFavourite } }` is one
  query rather than 500 connections and 500 table scans.

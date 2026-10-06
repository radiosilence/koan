- **The queue raced to fetch the same sleeve once per track.** Queue rows asked for artwork by track, so a twelve-track album was twelve HTTP round trips, twelve files on disk and twelve identical bitmaps in memory for one image. `QueueItem` now carries the album it came off — resolved for the whole queue in one query — and every row on a record shares a single fetch.

  This also closes a hole in the placeholder detection. Navidrome answers with a stock blue vinyl for anything with no artwork, and koan spots it by noticing the same image on three unrelated albums; lookups by track never took part in that vote, so the queue and the transport would draw the placeholder the grid had already learned to hide.


- **koan grew to a gigabyte of artwork and never gave any of it back.** Every cover you scrolled past was decoded and held for the life of the process — nothing evicted, so an hour of browsing a remote library reached 978 MB of decoded bitmaps, most of it swapped out rather than released. Covers are now held in a bounded cache that hands memory back under pressure.

  Bitmaps are also kept at the size they are drawn rather than the size they arrived. A queue row shows a sleeve at 28 points; it was holding the full 600-pixel image to do it, one per row. Rows and the transport keep a thumbnail, the grid and a record's own page keep a tile, and full size is decoded on demand for the artwork viewer and released with it — which is the only place the detail was ever visible.


package cc.blit.koan.state

import uniffi.koan_ffi.AlbumSort
import uniffi.koan_ffi.BrowseFilter
import uniffi.koan_ffi.TrackBrowseSort

/*
 * The browse sorts and filter, as `AlbumSort.swift` and `BrowseFilter.swift`
 * have them: stable strings for storage, so a stored preference survives the
 * enum gaining or reordering cases, and the labels the controls show.
 */

val BrowseFilterNone = BrowseFilter(
    favourites = false, recent = false, downloaded = false, lossless = false,
    codec = null, yearFrom = null, yearTo = null, genre = null,
)

/**
 * How many filters are on, the year range counting as one: what the control
 * shows, because a narrowed grid otherwise looks like a missing library.
 */
val BrowseFilter.activeCount: Int
    get() = listOf(
        favourites, recent, downloaded, lossless, codec != null, genre != null, yearFrom != null || yearTo != null,
    ).count { it }

/** As a store keeps it: `key=value` lines of what is set. */
fun BrowseFilter.stored(): String = buildList {
    if (favourites) add("favourites=1")
    if (recent) add("recent=1")
    if (downloaded) add("downloaded=1")
    if (lossless) add("lossless=1")
    codec?.let { add("codec=${escape(it)}") }
    genre?.let { add("genre=${escape(it)}") }
    yearFrom?.let { add("yearFrom=$it") }
    yearTo?.let { add("yearTo=$it") }
}.joinToString("\n")

fun browseFilterOf(stored: String?): BrowseFilter {
    val pairs = stored.orEmpty().lines().mapNotNull { line ->
        line.indexOf('=').takeIf { it > 0 }?.let { line.substring(0, it) to line.substring(it + 1) }
    }.toMap()
    return BrowseFilter(
        favourites = pairs["favourites"] == "1",
        recent = pairs["recent"] == "1",
        downloaded = pairs["downloaded"] == "1",
        lossless = pairs["lossless"] == "1",
        codec = pairs["codec"]?.let(::unescape),
        yearFrom = pairs["yearFrom"]?.toIntOrNull(),
        yearTo = pairs["yearTo"]?.toIntOrNull(),
        genre = pairs["genre"]?.let(::unescape),
    )
}

private fun escape(value: String) = value.replace("\\", "\\\\").replace("\n", "\\n")
private fun unescape(value: String) = buildString {
    var i = 0
    while (i < value.length) {
        val c = value[i]
        if (c == '\\' && i + 1 < value.length) {
            append(if (value[i + 1] == 'n') '\n' else value[i + 1])
            i += 2
        } else {
            append(c)
            i++
        }
    }
}

val AlbumSort.storageKey: String
    get() = when (this) {
        AlbumSort.RECENTLY_ADDED -> "recentlyAdded"
        AlbumSort.TITLE -> "title"
        AlbumSort.ARTIST -> "artist"
        AlbumSort.YEAR -> "year"
        AlbumSort.RANDOM -> "random"
        AlbumSort.LAST_PLAYED -> "lastPlayed"
        AlbumSort.DOWNLOADED -> "downloaded"
        AlbumSort.BEST_MATCH -> "bestMatch"
    }

fun albumSortOf(key: String?): AlbumSort? = AlbumSort.entries.firstOrNull { it.storageKey == key }

val AlbumSort.label: String
    get() = when (this) {
        AlbumSort.RECENTLY_ADDED -> "Recently Added"
        AlbumSort.TITLE -> "Title"
        AlbumSort.ARTIST -> "Artist"
        AlbumSort.YEAR -> "Year"
        AlbumSort.RANDOM -> "Random"
        AlbumSort.LAST_PLAYED -> "Last Played"
        AlbumSort.DOWNLOADED -> "Most Downloaded"
        AlbumSort.BEST_MATCH -> "Best Match"
    }

/**
 * The sorts to offer. Last Played only means something, and is only offered,
 * while the Recently Played filter is on; Most Downloaded while the Downloaded
 * one is; Best Match while a search narrows the listing.
 */
fun offeredAlbumSorts(recent: Boolean, downloaded: Boolean, searching: Boolean): List<AlbumSort> =
    (if (searching) listOf(AlbumSort.BEST_MATCH) else emptyList()) +
        (if (recent) listOf(AlbumSort.LAST_PLAYED) else emptyList()) +
        (if (downloaded) listOf(AlbumSort.DOWNLOADED) else emptyList()) +
        listOf(AlbumSort.RECENTLY_ADDED, AlbumSort.TITLE, AlbumSort.ARTIST, AlbumSort.YEAR, AlbumSort.RANDOM)

val TrackBrowseSort.storageKey: String
    get() = when (this) {
        TrackBrowseSort.ARTIST -> "artist"
        TrackBrowseSort.TITLE -> "title"
        TrackBrowseSort.ALBUM -> "album"
        TrackBrowseSort.DURATION -> "duration"
        TrackBrowseSort.LAST_PLAYED -> "lastPlayed"
        TrackBrowseSort.BEST_MATCH -> "bestMatch"
    }

fun trackSortOf(key: String?): TrackBrowseSort? = TrackBrowseSort.entries.firstOrNull { it.storageKey == key }

val TrackBrowseSort.label: String
    get() = when (this) {
        TrackBrowseSort.ARTIST -> "Artist"
        TrackBrowseSort.TITLE -> "Title"
        TrackBrowseSort.ALBUM -> "Album"
        TrackBrowseSort.DURATION -> "Duration"
        TrackBrowseSort.LAST_PLAYED -> "Last Played"
        TrackBrowseSort.BEST_MATCH -> "Best Match"
    }

fun offeredTrackSorts(recent: Boolean, searching: Boolean): List<TrackBrowseSort> =
    (if (searching) listOf(TrackBrowseSort.BEST_MATCH) else emptyList()) +
        (if (recent) listOf(TrackBrowseSort.LAST_PLAYED) else emptyList()) +
        listOf(TrackBrowseSort.ARTIST, TrackBrowseSort.TITLE, TrackBrowseSort.ALBUM, TrackBrowseSort.DURATION)

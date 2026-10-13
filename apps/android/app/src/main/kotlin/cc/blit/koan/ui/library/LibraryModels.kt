@file:OptIn(ExperimentalCoroutinesApi::class)

package cc.blit.koan.ui.library

import android.content.SharedPreferences
import androidx.core.content.edit
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.AlbumPage
import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.Artist
import cc.blit.koan.ffi.ArtistInfo
import cc.blit.koan.ffi.BrowseChoices
import cc.blit.koan.ffi.BrowseFilter
import cc.blit.koan.ffi.ShelfSummary
import cc.blit.koan.ffi.Track
import cc.blit.koan.ffi.TrackBrowseSort
import cc.blit.koan.ffi.TrackSort
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** Every filter off: the whole library. */
val NoFilter = BrowseFilter(
    favourites = false,
    recent = false,
    downloaded = false,
    lossless = false,
    codec = null,
    yearFrom = null,
    yearTo = null,
    genre = null,
)

/** How many filters are on, the year range counting as one. */
val BrowseFilter.activeCount
    get() = listOf(favourites, recent, downloaded, lossless, codec != null, genre != null, yearFrom != null || yearTo != null)
        .count { it }

/**
 * When a listing must be read again: whenever the library moves, and, while
 * it reads play history, whenever a play is recorded.
 */
private fun EngineMirror.versions(readsHistory: Flow<Boolean>) =
    combine(library, history, readsHistory) { v, h, r -> if (r) "$v:$h" else "$v" }.distinctUntilChanged()

private fun <T> ViewModel.held(flow: Flow<T>, initial: T): StateFlow<T> =
    flow.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), initial)

/** What a browser is narrowed and ordered by. */
data class BrowseQuery<S>(val sort: S, val filter: BrowseFilter, val search: String = "", val seed: Long = 0)

val AlbumSort.label
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
 * The sorts to offer. Last Played means something only while the Recently
 * Played filter is on, Most Downloaded while the Downloaded one is, Best Match
 * while a search narrows the listing.
 */
@JvmName("albumSorts")
fun BrowseQuery<AlbumSort>.offered() =
    (if (search.isNotBlank()) listOf(AlbumSort.BEST_MATCH) else emptyList()) +
        (if (filter.recent) listOf(AlbumSort.LAST_PLAYED) else emptyList()) +
        (if (filter.downloaded) listOf(AlbumSort.DOWNLOADED) else emptyList()) +
        listOf(AlbumSort.RECENTLY_ADDED, AlbumSort.TITLE, AlbumSort.ARTIST, AlbumSort.YEAR, AlbumSort.RANDOM)

val TrackBrowseSort.label
    get() = when (this) {
        TrackBrowseSort.ARTIST -> "Artist"
        TrackBrowseSort.TITLE -> "Title"
        TrackBrowseSort.ALBUM -> "Album"
        TrackBrowseSort.DURATION -> "Duration"
        TrackBrowseSort.LAST_PLAYED -> "Last Played"
        TrackBrowseSort.BEST_MATCH -> "Best Match"
    }

@JvmName("trackSorts")
fun BrowseQuery<TrackBrowseSort>.offered() =
    (if (search.isNotBlank()) listOf(TrackBrowseSort.BEST_MATCH) else emptyList()) +
        (if (filter.recent) listOf(TrackBrowseSort.LAST_PLAYED) else emptyList()) +
        listOf(TrackBrowseSort.ARTIST, TrackBrowseSort.TITLE, TrackBrowseSort.ALBUM, TrackBrowseSort.DURATION)

/** A shelf's filter and order, so the browser it opens lists what its preview is the head of. */
fun Shelf?.albumQuery(stored: AlbumSort) = when (this) {
    null -> BrowseQuery(stored, NoFilter)
    Shelf.Favourites -> BrowseQuery(AlbumSort.ARTIST, NoFilter.copy(favourites = true))
    Shelf.Recent -> BrowseQuery(AlbumSort.LAST_PLAYED, NoFilter.copy(recent = true))
    Shelf.Downloaded -> BrowseQuery(AlbumSort.DOWNLOADED, NoFilter.copy(downloaded = true))
    is Shelf.Search -> BrowseQuery(AlbumSort.BEST_MATCH, NoFilter, query)
}

fun Shelf.trackQuery() = when (this) {
    Shelf.Favourites -> BrowseQuery(TrackBrowseSort.ARTIST, NoFilter.copy(favourites = true))
    Shelf.Recent -> BrowseQuery(TrackBrowseSort.LAST_PLAYED, NoFilter.copy(recent = true))
    Shelf.Downloaded -> BrowseQuery(TrackBrowseSort.ARTIST, NoFilter.copy(downloaded = true))
    is Shelf.Search -> BrowseQuery(TrackBrowseSort.BEST_MATCH, NoFilter, query)
}

enum class AlbumLayout { Grid, List }

/**
 * The album browser. Narrowing and sorting happen in SQL; the listing arrives
 * whole, asked for again when the query or the library moves. The plain
 * browser remembers its sort and layout; one opened from a shelf starts from
 * the shelf's.
 */
class AlbumsModel(private val mirror: EngineMirror, private val shelf: Shelf?, private val prefs: SharedPreferences) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = Selection()

    val query = MutableStateFlow(
        shelf.albumQuery(prefs.getString(SORT, null)?.let { runCatching { AlbumSort.valueOf(it) }.getOrNull() } ?: AlbumSort.RECENTLY_ADDED),
    )

    val layout = MutableStateFlow(prefs.getString(LAYOUT, null)?.let { runCatching { AlbumLayout.valueOf(it) }.getOrNull() } ?: AlbumLayout.Grid)

    val albums: StateFlow<List<Album>?> = held(
        combine(mirror.versions(query.map { it.filter.recent }), query) { _, q -> q }
            .mapLatest { q ->
                runCatching { mirror.engine().albums(null, q.sort, q.seed, q.search.ifBlank { null }, q.filter) }.getOrDefault(emptyList())
            },
        null,
    )

    val choices: StateFlow<BrowseChoices?> = held(
        mirror.library.mapLatest { runCatching { mirror.engine().browseChoices() }.getOrNull() },
        null,
    )

    /** Choosing Random again deals a new shuffle. */
    fun sort(sort: AlbumSort) {
        query.update { it.copy(sort = sort, seed = if (sort == AlbumSort.RANDOM) System.nanoTime() else it.seed) }
        if (shelf == null) prefs.edit { putString(SORT, sort.name) }
    }

    fun filter(filter: BrowseFilter) = query.update { it.copy(filter = filter) }

    fun search(text: String) = query.update { it.copy(search = text) }

    fun layout(layout: AlbumLayout) {
        this.layout.value = layout
        prefs.edit { putString(LAYOUT, layout.name) }
    }

    private companion object {
        const val SORT = "albums.sort"
        const val LAYOUT = "albums.layout"
    }
}

class ArtistsModel(private val mirror: EngineMirror, shelf: Shelf?) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = Selection()

    val query = MutableStateFlow(shelf.albumQuery(AlbumSort.ARTIST))

    val artists: StateFlow<List<Artist>?> = held(
        combine(mirror.versions(query.map { it.filter.recent }), query) { _, q -> q }
            .mapLatest { q -> runCatching { mirror.engine().artists(q.search.ifBlank { null }, q.filter) }.getOrDefault(emptyList()) },
        null,
    )

    val choices: StateFlow<BrowseChoices?> = held(
        mirror.library.mapLatest { runCatching { mirror.engine().browseChoices() }.getOrNull() },
        null,
    )

    fun filter(filter: BrowseFilter) = query.update { it.copy(filter = filter) }

    fun search(text: String) = query.update { it.copy(search = text) }
}

data class TrackPage(val tracks: List<Track>, val total: Long)

/**
 * The track browser, read a page at a time: a shelf can hold tens of
 * thousands of tracks. A library change reads again as far as was read.
 */
class TracksModel(private val mirror: EngineMirror, shelf: Shelf) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = Selection()

    val query = MutableStateFlow(shelf.trackQuery())
    private val wanted = MutableStateFlow(PAGE)

    val page: StateFlow<TrackPage?> = held(
        combine(mirror.versions(query.map { it.filter.recent }), query, wanted) { _, q, n -> q to n }
            .mapLatest { (q, n) ->
                runCatching {
                    val listing = mirror.engine().trackListing(q.sort, q.search.ifBlank { null }, q.filter, n.toUInt(), 0u)
                    TrackPage(listing.tracks, listing.total.toLong())
                }.getOrNull()
            },
        null,
    )

    /** Read the next page, once the rows read so far are nearly all on screen. */
    fun more() {
        val held = page.value ?: return
        if (held.tracks.size < held.total && wanted.value <= held.tracks.size) wanted.update { it + PAGE }
    }

    fun sort(sort: TrackBrowseSort) = query.update { it.copy(sort = sort) }

    private companion object {
        const val PAGE = 500
    }
}

/** A shelf's page: the first few artists, records and tracks on it, and how many there are of each. */
class ShelfModel(mirror: EngineMirror, shelf: Shelf) : ViewModel() {
    val summary: StateFlow<ShelfSummary?> = held(
        mirror.versions(flowOf(shelf == Shelf.Recent))
            .mapLatest { runCatching { mirror.engine().shelfSummary(shelf.kind) }.getOrNull() },
        null,
    )
}

class AlbumModel(mirror: EngineMirror, albumId: Long) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = Selection()

    /** `null` while reading; a page whose `album` is `null` is a record no longer in the library. */
    val page: StateFlow<AlbumPage?> = held(
        mirror.library.mapLatest { runCatching { mirror.engine().albumPage(albumId) }.getOrNull() },
        null,
    )
}

data class ArtistRecord(
    val artist: Artist?,
    val albums: List<Album>,
    /** For an artist who owns no records: the tracks credited to them elsewhere. */
    val appearances: List<Track>,
)

/**
 * An artist's page: their records, oldest first, and their biography and
 * photograph. What is cached shows at once; the lookup over the network runs
 * once per visit, after the page is up.
 */
class ArtistModel(mirror: EngineMirror, artistId: Long) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = Selection()

    val record: StateFlow<ArtistRecord?> = held(
        mirror.library.mapLatest {
            runCatching {
                val engine = mirror.engine()
                val albums = engine.albums(artistId, AlbumSort.YEAR, 0, null, NoFilter)
                ArtistRecord(
                    engine.artist(artistId),
                    albums,
                    if (albums.isEmpty()) engine.tracks(null, artistId, TrackSort.ALBUM, 500u, 0u) else emptyList(),
                )
            }.getOrNull()
        },
        null,
    )

    private val _info = MutableStateFlow<ArtistInfo?>(null)
    val info: StateFlow<ArtistInfo?> = _info

    private val _looking = MutableStateFlow(false)

    /** A lookup is under way and nothing is cached to show meanwhile. */
    val looking: StateFlow<Boolean> = _looking

    init {
        viewModelScope.launch {
            val engine = mirror.engine()
            val cached = runCatching { engine.artistInfo(artistId) }.getOrNull()
            _info.value = cached
            _looking.value = cached == null
            val fetched = runCatching { engine.fetchArtistInfo(artistId) }.getOrNull()
            _looking.value = false
            if (fetched != null) _info.value = fetched
        }
    }
}

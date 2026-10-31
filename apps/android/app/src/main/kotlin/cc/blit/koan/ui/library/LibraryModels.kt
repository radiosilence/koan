@file:OptIn(ExperimentalCoroutinesApi::class)

package cc.blit.koan.ui.library

import android.content.SharedPreferences
import androidx.core.content.edit
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.engine.attempt
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.AlbumPage
import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.Artist
import cc.blit.koan.ffi.ArtistInfo
import cc.blit.koan.ffi.BrowseChoices
import cc.blit.koan.ffi.BrowseFilter
import cc.blit.koan.ffi.Track
import cc.blit.koan.ffi.TrackBrowseSort
import cc.blit.koan.ui.Listing
import cc.blit.koan.ui.read
import cc.blit.koan.ui.value
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

private fun <K, T> ViewModel.listing(keys: Flow<K>, fetch: suspend (K) -> T) = Listing(viewModelScope, keys, fetch)

/** The values the browsers' filters offer, read again when the library moves; none offered while they cannot be read. */
private fun ViewModel.choicesOf(mirror: EngineMirror): StateFlow<BrowseChoices?> =
    mirror.library.mapLatest { read { mirror.engine().browseChoices() }.value }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

/** What a browser is narrowed and ordered by. */
data class BrowseQuery<S>(val sort: S, val filter: BrowseFilter, val search: String = "", val seed: Long = 0)

val AlbumSort.label
    get() = when (this) {
        AlbumSort.RECENTLY_ADDED -> "Recently Added"
        AlbumSort.TITLE -> "Title"
        AlbumSort.ARTIST -> "Artist"
        AlbumSort.YEAR -> "Year, Newest"
        AlbumSort.YEAR_ASC -> "Year, Oldest"
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
        listOf(AlbumSort.RECENTLY_ADDED, AlbumSort.TITLE, AlbumSort.ARTIST, AlbumSort.YEAR, AlbumSort.YEAR_ASC, AlbumSort.RANDOM)

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
    val selection = playableSelection()

    val query = MutableStateFlow(
        shelf.albumQuery(prefs.getString(SORT, null)?.let { runCatching { AlbumSort.valueOf(it) }.getOrNull() } ?: AlbumSort.RECENTLY_ADDED),
    )

    val layout = MutableStateFlow(prefs.getString(LAYOUT, null)?.let { runCatching { AlbumLayout.valueOf(it) }.getOrNull() } ?: AlbumLayout.Grid)

    val albums = listing(combine(mirror.versions(query.map { it.filter.recent }), query) { _, q -> q }) { q ->
        mirror.engine().albums(null, q.sort, q.seed, q.search.ifBlank { null }, q.filter)
    }

    val choices = choicesOf(mirror)

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
    val selection = playableSelection()

    val query = MutableStateFlow(shelf.albumQuery(AlbumSort.ARTIST))

    val artists = listing(combine(mirror.versions(query.map { it.filter.recent }), query) { _, q -> q }) { q ->
        mirror.engine().artists(q.search.ifBlank { null }, q.filter)
    }

    val choices = choicesOf(mirror)

    fun filter(filter: BrowseFilter) = query.update { it.copy(filter = filter) }

    fun search(text: String) = query.update { it.copy(search = text) }
}

data class TrackPage(val tracks: List<Track>, val total: Long, val query: BrowseQuery<TrackBrowseSort>, val version: String = "") {
    /**
     * Where a read of the first `n` tracks of `query` at library `version`
     * starts, holding this page: after its tracks when it is the same listing
     * and short of `n`, else from the top.
     */
    fun readFrom(query: BrowseQuery<TrackBrowseSort>, version: String, n: Int) =
        if (query == this.query && version == this.version && tracks.size < n) tracks.size else 0
}

/**
 * The track browser, read a page at a time: a shelf can hold tens of
 * thousands of tracks. A library change reads again as far as was read.
 */
class TracksModel(private val mirror: EngineMirror, shelf: Shelf) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = playableSelection()

    val query = MutableStateFlow(shelf.trackQuery())
    private val wanted = MutableStateFlow(PAGE)

    /** The tracks read so far, which the next page is read after. Written only by the listing's reads, which run one at a time. */
    private var held: TrackPage? = null

    val page = listing(combine(mirror.versions(query.map { it.filter.recent }), query, wanted) { v, q, n -> Triple(v, q, n) }) { (v, q, n) ->
        val prior = held
        val from = prior?.readFrom(q, v, n) ?: 0
        val listing = mirror.engine().trackListing(q.sort, q.search.ifBlank { null }, q.filter, (n - from).toUInt(), from.toUInt())
        val tracks = if (from > 0 && prior != null) prior.tracks + listing.tracks else listing.tracks
        TrackPage(tracks, listing.total.toLong(), q, v).also { held = it }
    }

    /** Read the next page, once the rows read so far are nearly all on screen. */
    fun more() {
        // Not until the new query's first page has arrived.
        val held = page.state.value.value?.takeIf { it.query == query.value } ?: return
        if (held.tracks.size < held.total && wanted.value <= held.tracks.size) wanted.update { it + PAGE }
    }

    val choices = choicesOf(mirror)

    fun sort(sort: TrackBrowseSort) = requery { it.copy(sort = sort) }

    fun filter(filter: BrowseFilter) = requery { it.copy(filter = filter) }

    fun search(text: String) = requery { it.copy(search = text) }

    /**
     * A new query starts again from its first page, however far the last one
     * was read, and leaves a sort it no longer offers, such as Best Match once
     * the search is cleared.
     */
    private fun requery(change: (BrowseQuery<TrackBrowseSort>) -> BrowseQuery<TrackBrowseSort>) {
        wanted.value = PAGE
        query.update { change(it).let { q -> if (q.sort in q.offered()) q else q.copy(sort = q.offered().first()) } }
    }

    private companion object {
        const val PAGE = 500
    }
}

/** A shelf's page: the first few artists, records and tracks on it, and how many there are of each. */
class ShelfModel(mirror: EngineMirror, shelf: Shelf) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = playableSelection()

    val summary = listing(mirror.versions(flowOf(shelf == Shelf.Recent))) { mirror.engine().shelfSummary(shelf.kind) }
}

/**
 * A record's page, read again when the library moves or when asked to after a
 * failed read. A failure is said as one, never left as a page still reading;
 * a page read with no `album` is a record that has left the library.
 */
class AlbumModel(fetch: suspend () -> AlbumPage, library: Flow<Long>) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = playableSelection()

    val page = listing(library) { fetch() }
}

data class ArtistRecord(
    val artist: Artist?,
    /** The records they are the album artist of: what the page's Play plays. */
    val albums: List<Album>,
    /** The records they are credited on and someone else is the album artist of, ordered as [albums] are. */
    val features: List<Album>,
    /** For an artist who owns no records and features on none listed: the tracks credited to them. */
    val appearances: List<Track>,
)

/**
 * An artist's page: their records, oldest first, and their biography and
 * photograph. What is cached shows at once; the lookup over the network runs
 * once per visit, after the page is up.
 */
class ArtistModel(mirror: EngineMirror, artistId: Long) : ViewModel() {
    /** Things picked to play or queue together; ends with the page. */
    val selection = playableSelection()

    val record = listing(mirror.library) {
        val engine = mirror.engine()
        val albums = engine.albums(artistId, AlbumSort.YEAR, 0, null, NoFilter)
        ArtistRecord(
            engine.artist(artistId),
            albums,
            engine.artistFeatures(artistId),
            if (albums.isEmpty()) engine.tracks(null, artistId) else emptyList(),
        )
    }

    private val _info = MutableStateFlow<ArtistInfo?>(null)
    val info: StateFlow<ArtistInfo?> = _info

    private val _looking = MutableStateFlow(false)

    /** A lookup is under way and nothing is cached to show meanwhile. */
    val looking: StateFlow<Boolean> = _looking

    init {
        viewModelScope.launch {
            val engine = mirror.engine()
            // A biography is extra to the page: one that cannot be read is left out.
            val cached = attempt { engine.artistInfo(artistId) }.getOrNull()
            _info.value = cached
            _looking.value = cached == null
            val fetched = attempt { engine.fetchArtistInfo(artistId) }.getOrNull()
            _looking.value = false
            if (fetched != null) _info.value = fetched
        }
    }
}

package cc.blit.koan.state

import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.art.ArtworkWarmer
import cc.blit.koan.nav.PageLoader
import cc.blit.koan.nav.Route
import cc.blit.koan.nav.Section
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.koan_ffi.Album
import uniffi.koan_ffi.AlbumSort
import uniffi.koan_ffi.Artist
import uniffi.koan_ffi.ArtistInfo
import uniffi.koan_ffi.BrowseChoices
import uniffi.koan_ffi.BrowseFilter
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.PlayHistoryEntry
import uniffi.koan_ffi.ProgressReporter
import uniffi.koan_ffi.ScanSummary
import uniffi.koan_ffi.ShelfKind
import uniffi.koan_ffi.ShelfSummary
import uniffi.koan_ffi.Stats
import uniffi.koan_ffi.Track
import uniffi.koan_ffi.TrackBrowseSort
import uniffi.koan_ffi.TrackListing
import uniffi.koan_ffi.TrackSort
import kotlin.random.Random

/**
 * A section and everything it shows, as one value.
 *
 * Read before the navigator moves (see [LibraryModel.prepare]). A section that
 * arrives first and asks afterwards draws itself empty, and the empty state of
 * a listing is the word "No albums yet" over a page that has albums.
 */
class Listing internal constructor(
    val section: Section,
    /** The name filter it was read with: empty, unless a shelf's heading carried one in. */
    val filter: String,
    internal val rows: Rows,
)

internal sealed interface Rows {
    data object None : Rows
    data class Albums(val rows: List<Album>) : Rows
    data class Artists(val rows: List<Artist>) : Rows
    data class Shelf(val summary: ShelfSummary) : Rows
    data class History(val rows: List<PlayHistoryEntry>) : Rows
    data class Tracks(val listing: TrackListing) : Rows
}

/** Which listing a shelf section opens. */
enum class ShelfList(val section: Section) {
    Artists(Section.Artists),
    Albums(Section.Albums),
    Tracks(Section.Tracks),
}

/** A record and its tracks, as one value, by album. */
data class AlbumRecord(
    val albumId: Long,
    /**
     * The library version it was read at. What makes asking for the record
     * already on screen free, and asking for it after the rows moved a real read.
     */
    val stamp: Long,
    val album: Album?,
    val tracks: List<Track>,
    /** The read itself failed, as opposed to finding nothing. */
    val failed: Boolean = false,
)

/** An artist, their records and who they sound like, as one value. */
data class ArtistRecord(
    val artistId: Long,
    val stamp: Long,
    val artist: Artist?,
    val albums: List<Album>,
    /** For an artist who owns no albums (a guest, a compilation track): the tracks credited to them, which are then the whole page. */
    val appearances: List<Track> = emptyList(),
    /** Biography and photograph, as cached; filled in from the network after the page is up. */
    val info: ArtistInfo? = null,
    /** A network lookup is under way and nothing is cached to show yet. */
    val infoLoading: Boolean = false,
)

/**
 * Library browsing state. The port of `LibraryModel.swift`; its doc comments
 * are the specification, and the ones that matter here are these.
 *
 * Nothing here is derived, indexed or narrowed. A section asks koan-core what
 * it should be showing and shows exactly that; narrowing and sorting happen in
 * SQL, because the database is the only thing that knows the answer and asking
 * it is cheaper than keeping one.
 *
 * Nothing is paged either, bar the track browser, which reads a thousand at a
 * time because it can be hundreds of thousands. Every other listing arrives
 * whole, so the scrollbar tells the truth about how long the library is. There
 * is no load to have forgotten to do: a section never visited shows the library
 * the first time it is, and one whose rows changed underneath it asks again
 * rather than merging.
 *
 * **Threading.** Everything is confined to [scope]'s dispatcher, which is the
 * engine's and never Main. Every public method returns at once and does its
 * work there; a composable reads the flows and calls methods, and never the
 * engine. A listing is published only where it differs from the one on
 * screen: a flow dedupes by equality, so the same answer as last time (the
 * common case, since every library version bump reloads) wakes nobody and
 * costs one walk over the rows.
 */
class LibraryModel(
    private val engine: KoanEngineInterface,
    private val mirror: EngineMirror,
    private val scope: CoroutineScope,
    private val store: KeyValueStore = KeyValueStore.Memory(),
    private val activity: ActivityModel? = null,
    private val art: ArtworkWarmer? = null,
    private val filterDebounceMs: Long = 80,
    private val random: Random = Random.Default,
) : PageLoader {
    private val _section = MutableStateFlow<Section>(Section.Queue)
    private val _filter = MutableStateFlow("")
    private val _albumSort = MutableStateFlow(AlbumSort.RECENTLY_ADDED)
    private val _trackSort = MutableStateFlow(TrackBrowseSort.ARTIST)
    private val _browseFilter = MutableStateFlow(BrowseFilterNone)
    private val _browseChoices = MutableStateFlow<BrowseChoices?>(null)
    private val _albums = MutableStateFlow<List<Album>>(emptyList())
    private val _artists = MutableStateFlow<List<Artist>>(emptyList())
    private val _history = MutableStateFlow<List<PlayHistoryEntry>>(emptyList())
    private val _shelf = MutableStateFlow<ShelfSummary?>(null)
    private val _tracks = MutableStateFlow<List<Track>>(emptyList())
    private val _trackTotal = MutableStateFlow(0L)
    private val _favouriteTracks = MutableStateFlow<Set<Long>>(emptySet())
    private val _favouriteAlbums = MutableStateFlow<Set<Long>>(emptySet())
    private val _favouriteArtists = MutableStateFlow<Set<Long>>(emptySet())
    private val _stats = MutableStateFlow<Stats?>(null)
    private val _signedIn = MutableStateFlow<Boolean?>(null)
    private val _loading = MutableStateFlow(false)
    private val _scanSummary = MutableStateFlow<ScanSummary?>(null)
    private val _records = MutableStateFlow<Map<Long, AlbumRecord>>(emptyMap())
    private val _artist = MutableStateFlow<ArtistRecord?>(null)

    private var loading: Job? = null
    private var shuffleSeed = random.nextLong()

    /** The name filter the next prepared section is read with. Set by a See all, spent by the move it makes. */
    private var carried: String? = null

    /** True while a shelf's heading sets the browser up: one change, asking for nothing until the move it precedes. */
    private var seeding = false

    /** True while a prepared listing is adopted: emptying the filter it arrives with is not a reason to ask again. */
    private var adopting = false

    /** The albums in [records], least recently read first. Past [HELD_RECORDS] the oldest goes. */
    private val recordOrder = ArrayList<Long>()

    init {
        albumSortOf(store.getString(KEY_ALBUM_SORT))?.let { _albumSort.value = it }
        trackSortOf(store.getString(KEY_TRACK_SORT))?.let { _trackSort.value = it }
        _browseFilter.value = browseFilterOf(store.getString(KEY_BROWSE_FILTER))
    }

    // MARK: - What is on screen

    /** What is on screen. Written only by the navigator, which owns it: the library follows where you are, it does not decide it. */
    val section: StateFlow<Section> = _section.asStateFlow()

    /** Substring filter over whatever the current section is showing. It narrows the query, not the answer. */
    val filter: StateFlow<String> = _filter.asStateFlow()

    /** Newest first by default: the record you just added is the one you are looking for. Persisted. */
    val albumSort: StateFlow<AlbumSort> = _albumSort.asStateFlow()

    /** How the track browser is ordered. Persisted as the album sort is. */
    val trackSort: StateFlow<TrackBrowseSort> = _trackSort.asStateFlow()

    /**
     * What the album and artist browsers are narrowed to beyond the name
     * filter. Unlike the name filter it follows you between the two and
     * survives a relaunch, as the sort does: a standing choice.
     */
    val browseFilter: StateFlow<BrowseFilter> = _browseFilter.asStateFlow()

    /** What the codec and genre filters offer, read when the filters open. */
    val browseChoices: StateFlow<BrowseChoices?> = _browseChoices.asStateFlow()

    /** The section's rows, as the database handed them over. One flow per kind: a page collects the one it draws. */
    val visibleAlbums: StateFlow<List<Album>> = _albums.asStateFlow()
    val visibleArtists: StateFlow<List<Artist>> = _artists.asStateFlow()
    val visiblePlayHistory: StateFlow<List<PlayHistoryEntry>> = _history.asStateFlow()

    /** The shelf on screen: Favourites' or Recently Played's previews and how many there are of each. */
    val visibleShelf: StateFlow<ShelfSummary?> = _shelf.asStateFlow()

    /** The track browser's rows so far, and how many it will have. */
    val visibleTracks: StateFlow<List<Track>> = _tracks.asStateFlow()
    val trackTotal: StateFlow<Long> = _trackTotal.asStateFlow()

    /**
     * Favourite state is read from here rather than from the copy baked into
     * each track when it was fetched. A track appears in the album view, the
     * artist view, the queue and search results, and refetching every one of
     * those after a heart tap is neither cheap nor reliable.
     */
    val favouriteTrackIds: StateFlow<Set<Long>> = _favouriteTracks.asStateFlow()
    val favouriteAlbumIds: StateFlow<Set<Long>> = _favouriteAlbums.asStateFlow()
    val favouriteArtistIds: StateFlow<Set<Long>> = _favouriteArtists.asStateFlow()

    val stats: StateFlow<Stats?> = _stats.asStateFlow()

    /** Whether a server and its credential are set, loaded with [stats]: an empty library means something different signed in and signed out. */
    val signedIn: StateFlow<Boolean?> = _signedIn.asStateFlow()
    val isLoading: StateFlow<Boolean> = _loading.asStateFlow()
    val scanSummary: StateFlow<ScanSummary?> = _scanSummary.asStateFlow()

    /** Each record page's record and tracks, held by album: several record pages are alive at once, one in each tab's stack. */
    val detailRecords: StateFlow<Map<Long, AlbumRecord>> = _records.asStateFlow()

    /** The artist page's record. */
    val detailArtist: StateFlow<ArtistRecord?> = _artist.asStateFlow()

    /** Whether the browser on screen is showing less than the whole library. */
    val isNarrowed: Boolean get() = _filter.value.isNotEmpty() || _browseFilter.value.activeCount > 0

    /** What an empty page says on a phone, whose library is a server's. */
    fun emptyLibraryDetail(signedIn: Boolean?): String =
        if (signedIn == true) {
            "Nothing from your server yet. It appears here once kōan has synced; Settings → Server shows how that is going."
        } else {
            "Sign in to your music server in Settings → Server."
        }

    // MARK: - Moving

    /**
     * What a section will be showing, without touching what is on screen. Null
     * when it is already showing: the rows are in hand and the filter over them
     * is somebody's, so a move back onto a section is not a reason to re-read
     * it or to throw their narrowing away. A library change reloads it where it
     * is drawn instead.
     */
    suspend fun prepare(section: Section): Listing? {
        if (section == _section.value) return null
        // Nothing carries over: a filter you left behind on another view is
        // invisible here, and an apparently empty library is the result. The
        // one exception is a shelf's heading, which arrives with the shelf's query.
        val filter = carried.orEmpty()
        carried = null
        return Listing(section, filter, request(section, filter).rows())
    }

    /** Adopt a listing, at the moment the navigator moves to it. */
    fun show(listing: Listing) {
        loading?.cancel()
        _section.value = listing.section
        adopting = true
        _filter.value = listing.filter
        adopting = false
        publish(listing.rows)
        _loading.value = false
        loading = scope.launch { continueTracks(request(), _trackTotal.value) }
    }

    // The navigator's seam: a section is a listing, a record or an artist is a read.
    override suspend fun prepare(route: Route): Any? = when (route) {
        is Route.Page -> prepare(route.section)
        is Route.Album -> {
            prepareAlbum(route.id)
            null
        }
        is Route.Artist -> {
            prepareArtist(route.id)
            null
        }
        Route.Library, Route.Settings, Route.Playlists -> null
    }

    override fun arrive(route: Route, prepared: Any?) {
        if (prepared is Listing) show(prepared)
    }

    /**
     * Point a browser at a shelf: its filters and sort set to the shelf's, so
     * the listing it opens on is the one the shelf's preview is the head of and
     * its count is the one the shelf gave. Returns the section to show. The
     * filters replace whatever was set, as a link does, and stay set.
     */
    fun browse(list: ShelfList, shelf: ShelfKind): Section {
        seeding = true
        try {
            var filter = BrowseFilterNone
            when (shelf) {
                ShelfKind.Favourites -> {
                    filter = filter.copy(favourites = true)
                    setAlbumSortNow(AlbumSort.ARTIST)
                    setTrackSortNow(TrackBrowseSort.ARTIST)
                }
                ShelfKind.Recent -> {
                    filter = filter.copy(recent = true)
                    setAlbumSortNow(AlbumSort.LAST_PLAYED)
                    setTrackSortNow(TrackBrowseSort.LAST_PLAYED)
                }
                is ShelfKind.Search -> {
                    carried = shelf.query
                    setAlbumSortNow(AlbumSort.BEST_MATCH)
                    setTrackSortNow(TrackBrowseSort.BEST_MATCH)
                }
                ShelfKind.Downloaded -> {
                    filter = filter.copy(downloaded = true)
                    setAlbumSortNow(AlbumSort.DOWNLOADED)
                    setTrackSortNow(TrackBrowseSort.ARTIST)
                }
            }
            setBrowseFilterNow(filter)
        } finally {
            seeding = false
        }
        return list.section
    }

    // MARK: - Narrowing and sorting

    fun setFilter(text: String) {
        scope.launch {
            if (text == _filter.value) return@launch
            _filter.value = text
            if (!adopting) reload(debounced = true)
        }
    }

    fun setAlbumSort(sort: AlbumSort) {
        scope.launch {
            if (setAlbumSortNow(sort) && !seeding) reload()
        }
    }

    fun setTrackSort(sort: TrackBrowseSort) {
        scope.launch {
            if (setTrackSortNow(sort) && !seeding && _section.value == Section.Tracks) reload()
        }
    }

    fun setBrowseFilter(filter: BrowseFilter) {
        scope.launch {
            if (setBrowseFilterNow(filter) && !seeding && _section.value.isBrowser) reload()
        }
    }

    private fun setAlbumSortNow(sort: AlbumSort): Boolean {
        if (sort == _albumSort.value) return false
        _albumSort.value = sort
        store.putString(KEY_ALBUM_SORT, sort.storageKey)
        return true
    }

    private fun setTrackSortNow(sort: TrackBrowseSort): Boolean {
        if (sort == _trackSort.value) return false
        _trackSort.value = sort
        store.putString(KEY_TRACK_SORT, sort.storageKey)
        return true
    }

    private fun setBrowseFilterNow(filter: BrowseFilter): Boolean {
        if (filter == _browseFilter.value) return false
        _browseFilter.value = filter
        store.putString(KEY_BROWSE_FILTER, filter.stored())
        return true
    }

    fun loadBrowseChoices() {
        scope.launch {
            runCatchingEngine { engine.browseChoices() }?.let { _browseChoices.value = it }
        }
    }

    /**
     * Deal again. Held rather than dealt afresh on every read, so typing in the
     * filter narrows the shuffle you are looking at instead of dealing a new one
     * on each keystroke. Only visibly different under Random, which is what the
     * button is for.
     */
    fun reshuffleAlbums() {
        scope.launch {
            shuffleSeed = random.nextLong()
            reload()
        }
    }

    // MARK: - Loading

    /**
     * Ask for whatever is on screen. Cancellable, so an answer to a filter you
     * have already typed past never lands, and debounced when a keystroke caused
     * it, so holding a key down is one query rather than one per character.
     * Call on the model's dispatcher.
     */
    private fun reload(debounced: Boolean = false) {
        _loading.value = true
        loading?.cancel()
        val request = request()
        loading = scope.launch {
            if (debounced) delay(filterDebounceMs)
            val rows = request.rows()
            ensureActive()
            publish(rows)
            _loading.value = false
            continueTracks(request, _trackTotal.value)
        }
    }

    private fun request(section: Section = _section.value, filter: String = _filter.value) = Request(
        engine, section, filter, _browseFilter.value, _albumSort.value, _trackSort.value, shuffleSeed,
    )

    /**
     * The rest of the track browser, a page at a time after the first, each
     * appended as it lands. Part of the load it follows: anything that reloads
     * cancels it, so a page of the old listing never lands on the new.
     */
    private suspend fun continueTracks(request: Request, total: Long) {
        if (request.section != Section.Tracks) return
        var offset = _tracks.value.size
        while (offset < total) {
            val page = request.tracks(offset.toUInt()) ?: return
            if (page.tracks.isEmpty()) return
            currentCoroutineContext().ensureActive()
            _tracks.update { it + page.tracks }
            offset += page.tracks.size
        }
    }

    /**
     * Publish what came back, and only where it differs from what is already
     * on screen. A flow dedupes by equality; the walk over the rows is the cost.
     */
    private fun publish(rows: Rows) {
        when (rows) {
            Rows.None -> Unit
            is Rows.Albums -> _albums.value = rows.rows
            is Rows.Artists -> _artists.value = rows.rows
            is Rows.Shelf -> _shelf.value = rows.summary
            is Rows.History -> _history.value = rows.rows
            is Rows.Tracks -> {
                _trackTotal.value = rows.listing.total.toLong()
                _tracks.value = rows.listing.tracks
            }
        }
    }

    // MARK: - History and stats

    /** Forget specific plays. The tracks are untouched; only the log changes. */
    fun forgetPlays(ids: Set<Long>) {
        if (ids.isEmpty()) return
        scope.launch {
            // Dropped locally first so the list does not visibly lag the tap.
            _history.update { rows -> rows.filterNot { it.id in ids } }
            runCatchingEngine { engine.deletePlays(ids.toList()) }
        }
    }

    /** Forget every play. */
    fun clearPlayHistory() {
        scope.launch {
            runCatchingEngine { engine.clearPlayHistory() }
            _history.value = emptyList()
        }
    }

    fun loadStats() {
        scope.launch {
            _stats.value = runCatchingEngine { engine.libraryStats() } ?: _stats.value
            _signedIn.value = runCatchingEngine { engine.settings().remoteSignedIn } ?: _signedIn.value
        }
    }

    // MARK: - Records

    /**
     * Read the record and its tracks, both at once, before the page appears. A
     * page that fetches once it is already on screen has to draw itself empty
     * first. Both halves land together or not at all, so the header can never
     * arrive ahead of the rows either.
     *
     * Asking for the record already in hand, with nothing changed under it, is
     * free: the navigator loads a record before it moves to it, so the page's
     * own reload asks again the moment it appears.
     */
    suspend fun prepareAlbum(id: Long) {
        val stamp = mirror.libraryVersion.value
        _records.value[id]?.let { held -> if (held.stamp == stamp && !held.failed) return }
        // The sleeve and the colour the room takes from it, never waited on:
        // arriving cold this is an HTTP round trip.
        warm(ArtworkSource.Album(id))
        val loaded = try {
            val page = engine.albumPage(id)
            AlbumRecord(id, stamp, page.album, page.tracks)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            AlbumRecord(id, stamp, null, emptyList(), failed = true)
        }
        val held = _records.value[id]
        // Reads finish in any order; one begun before the library last moved
        // never replaces one begun after. A failed read never replaces a good
        // record: the page keeps what it was showing.
        if (held != null && !held.failed && (held.stamp > loaded.stamp || loaded.failed)) return
        recordOrder.remove(id)
        recordOrder.add(id)
        _records.update { it + (id to loaded) }
        if (recordOrder.size > HELD_RECORDS) {
            val oldest = recordOrder.removeAt(0)
            _records.update { it - oldest }
        }
    }

    /** Read an artist and everything their page draws, as independent queries, all at once. */
    suspend fun prepareArtist(id: Long) {
        val stamp = mirror.libraryVersion.value
        _artist.value?.let { held -> if (held.artistId == id && held.stamp == stamp) return }
        val record = coroutineScope {
            val artist = async { runCatchingEngine { engine.artist(id) } }
            val albums = async {
                runCatchingEngine { engine.albums(id, AlbumSort.YEAR, 0L, null, BrowseFilterNone) }.orEmpty()
            }
            val info = async { runCatchingEngine { engine.artistInfo(id) } }
            val owned = albums.await()
            // Read only when there is nothing else to show: an artist with
            // albums has their tracks one tap away on each.
            val appearances = if (owned.isEmpty()) {
                runCatchingEngine { engine.tracks(null, id, TrackSort.ALBUM, 500u, 0u) }.orEmpty()
            } else {
                emptyList()
            }
            val cached = info.await()
            ArtistRecord(id, stamp, artist.await(), owned, appearances, cached, infoLoading = cached == null)
        }
        _artist.value = record
        enrichArtist(id)
    }

    /**
     * Ask for the artist's biography and photograph, and fold them into the page
     * when they land. Never waited on: a miss is several seconds of MusicBrainz
     * and Wikipedia, and the page is already drawn from the cache.
     */
    private fun enrichArtist(id: Long) {
        scope.launch {
            val fetched = runCatchingEngine { engine.fetchArtistInfo(id) }
            val held = _artist.value
            if (held == null || held.artistId != id) return@launch
            _artist.value = held.copy(infoLoading = false, info = fetched ?: held.info)
        }
    }

    /**
     * Put this record's sleeve in the cache, if it is not there already.
     * Never waited on.
     */
    fun warm(source: ArtworkSource) {
        art?.warm(source)
    }

    // MARK: - Favourites

    /**
     * Toggle a track favourite and reflect it everywhere at once. The engine
     * returns the new state, so the id sets are updated from that rather than by
     * re-reading the database: the row responds on the tap rather than a round
     * trip later.
     */
    fun toggleFavouriteTrack(id: Long) = toggle(id, _favouriteTracks) { engine.toggleFavourite(it) }

    fun toggleFavouriteAlbum(id: Long) = toggle(id, _favouriteAlbums) { engine.toggleFavouriteAlbum(it) }

    fun toggleFavouriteArtist(id: Long) = toggle(id, _favouriteArtists) { engine.toggleFavouriteArtist(it) }

    private fun toggle(id: Long, ids: MutableStateFlow<Set<Long>>, ask: suspend (Long) -> Boolean) {
        scope.launch {
            val now = runCatchingEngine { ask(id) } ?: return@launch
            ids.update { if (now) it + id else it - id }
            reloadFavourites()
        }
    }

    /** Re-read every favourite id. Called after a sync, which can change them without going through a toggle. */
    fun refreshFavourites() {
        scope.launch {
            // Three independent reads, so three at once.
            val (tracks, albums, artists) = coroutineScope {
                val t = async { runCatchingEngine { engine.favouriteTrackIds() }.orEmpty().toSet() }
                val a = async { runCatchingEngine { engine.favouriteAlbumIds() }.orEmpty().toSet() }
                val r = async { runCatchingEngine { engine.favouriteArtistIds() }.orEmpty().toSet() }
                Triple(t.await(), a.await(), r.await())
            }
            // A flow does not wake for an equal set, and every heart on screen
            // reads these: a sync reconciling favourites usually finds them all the same.
            _favouriteTracks.value = tracks
            _favouriteAlbums.value = albums
            _favouriteArtists.value = artists
            reloadFavourites()
        }
    }

    /** The favourites page lists what the hearts say, so a toggle changes it. */
    private fun reloadFavourites() {
        val section = _section.value
        if (section == Section.Favourites || (section.isBrowser && _browseFilter.value.favourites)) reload()
    }

    // MARK: - Library tasks

    /** Pull the remote library. Minutes on a large server, so it runs detached; the engine announces the rows it wrote. */
    fun syncRemote() {
        if (activity?.conflicts(setOf(Resource.RemoteTracks)) == true) return
        scope.launch {
            tracked("Syncing with server", setOf(Resource.RemoteTracks), followsSync = true) {
                runCatchingEngine { engine.syncRemote() }
            }
        }
    }

    /**
     * Throw away every file cached from the server. The library rows stay, so
     * the tracks remain playable and simply download again on demand.
     */
    fun clearDownloads() {
        if (activity?.conflicts(setOf(Resource.Downloads)) == true) return
        scope.launch {
            tracked("Clearing downloaded files", setOf(Resource.Downloads)) {
                runCatchingEngine { engine.clearDownloadCache() }
            }
            loadStats()
        }
    }

    /** Throw away the downloaded copies of these tracks. Claims nothing: it touches only the rows named. */
    fun clearDownloads(trackIds: List<Long>) {
        if (trackIds.isEmpty()) return
        scope.launch {
            runCatchingEngine { engine.clearDownloads(trackIds) }
            loadStats()
        }
    }

    /** Fetch these tracks into the cache without queueing them. Tracks already downloaded are skipped. */
    fun downloadToCache(trackIds: List<Long>) {
        if (trackIds.isEmpty()) return
        scope.launch { runCatchingEngine { engine.downloadToCache(trackIds) } }
    }

    /** Full rescan of every configured folder. Minutes on a big library, so it runs detached and the UI stays live. */
    fun scan(force: Boolean = false) {
        if (activity?.conflicts(Resource.LocalLibrary) == true) return
        _scanSummary.value = null
        scope.launch {
            val label = if (force) "Rescanning every file" else "Scanning library"
            _scanSummary.value = tracked(label, Resource.LocalLibrary, cancellable = true) { progress ->
                runCatchingEngine { engine.scanReporting(force, progress) }
            }
        }
    }

    /**
     * Rows appeared or vanished underneath us: a scan, a sync, an import, a
     * playlist edit, a download landing, a folder being forgotten. Whether this
     * app asked for it or the engine did it on its own makes no difference
     * here: nothing to merge, nothing to invalidate, just ask again. Favourites
     * too, because a sync reconciles them with the server. The section's rows
     * only; what a page is showing reloads where it is drawn.
     */
    fun libraryChanged() {
        loadStats()
        refreshFavourites()
        scope.launch { reload() }
    }

    /** A play was recorded or forgotten: the sections derived from history ask again, and nothing else does. */
    fun historyChanged() {
        scope.launch {
            val section = _section.value
            val derived = section == Section.RecentlyPlayed || section == Section.PlayHistory ||
                (section.isBrowser && _browseFilter.value.recent)
            if (derived) reload()
        }
    }

    /** [body] with a row in the activity registry for as long as it runs (if there is a registry). */
    private suspend fun <T> tracked(
        label: String,
        uses: Set<Resource>,
        cancellable: Boolean = false,
        followsSync: Boolean = false,
        body: suspend (ProgressReporter?) -> T,
    ): T {
        val registry = activity ?: return body(null)
        val id = registry.begin(label, uses, cancellable, followsSync)
        try {
            return body(registry.reporter(id))
        } finally {
            registry.end(id)
        }
    }

    private suspend fun <T> runCatchingEngine(call: suspend () -> T): T? = try {
        call()
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        null
    }

    private companion object {
        const val KEY_ALBUM_SORT = "albumSort"
        const val KEY_TRACK_SORT = "trackSort"
        const val KEY_BROWSE_FILTER = "browseFilter"

        /** Past this the oldest record read goes; a page whose record went reads it again. */
        const val HELD_RECORDS = 64
    }
}

/**
 * Everything a section's query depends on, captured off the model so the
 * answer that lands belongs to the question that was asked. Anything that
 * changes one cancels the job holding it.
 */
internal class Request(
    private val engine: KoanEngineInterface,
    val section: Section,
    private val filter: String,
    private val browse: BrowseFilter,
    private val sort: AlbumSort,
    private val trackSort: TrackBrowseSort,
    private val seed: Long,
) {
    private val search: String? get() = filter.ifEmpty { null }

    /** A page of the track browser. */
    suspend fun tracks(offset: UInt): TrackListing? = try {
        engine.trackListing(trackSort, search, browse, TRACK_PAGE, offset)
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        null
    }

    /** Everything this section is showing. */
    suspend fun rows(): Rows = try {
        when (section) {
            Section.Queue, Section.SearchResults, is Section.Playlist, Section.Downloads ->
                // Owned by the player, search, playlist and downloads models respectively.
                Rows.None
            Section.Albums -> Rows.Albums(engine.albums(null, sort, seed, search, browse))
            Section.Artists -> Rows.Artists(engine.artists(search, browse))
            Section.Tracks -> Rows.Tracks(engine.trackListing(trackSort, search, browse, TRACK_PAGE, 0u))
            Section.Favourites -> Rows.Shelf(engine.shelfSummary(ShelfKind.Favourites))
            Section.PlayHistory -> Rows.History(engine.playHistory(search))
            Section.RecentlyPlayed -> Rows.Shelf(engine.shelfSummary(ShelfKind.Recent))
            Section.OnDevice -> Rows.Shelf(engine.shelfSummary(ShelfKind.Downloaded))
        }
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        // A failed read shows what was there; the page does not blank.
        Rows.None
    }

    companion object {
        /** How many tracks the browser reads at once. */
        const val TRACK_PAGE: UInt = 1000u
    }
}

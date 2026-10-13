package cc.blit.koan.state

import cc.blit.koan.nav.Navigator
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import uniffi.koan_ffi.Album
import uniffi.koan_ffi.Artist
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.ShelfKind
import uniffi.koan_ffi.Track

/** The answer to the query on screen, as one value. */
data class SearchResults(
    /** The text these answer; empty before the first answer. */
    val query: String = "",
    val artists: List<Artist> = emptyList(),
    val albums: List<Album> = emptyList(),
    val tracks: List<Track> = emptyList(),
    val totals: ShelfTotals? = null,
    /** A query is typed and its answer has not landed. */
    val searching: Boolean = false,
) {
    val isEmpty: Boolean get() = artists.isEmpty() && albums.isEmpty() && tracks.isEmpty()

    fun playables(): List<Playable> =
        artists.map { Playable.OfArtist(it.id, it.name) } + albums.map(Playable::OfAlbum) + tracks.map(Playable::OfTrack)
}

/**
 * Global search across artists, albums and tracks. The port of
 * `SearchModel.swift`; its doc comments are the specification.
 *
 * One query, three result sets: the shape every streaming client uses, and the
 * reason there is no separate per-view filter. The results are the search
 * shelf's (`koan_core::shelves`): each section is the head of the listing its
 * heading's "See all" opens, and its total that listing's length. The closest
 * matches come first; a query nothing holds as typed falls back to fuzzy
 * matching, so a typo still finds what was meant.
 *
 * **Differences from the Mac.** The Search tab's root is the results page, so
 * there is no page to move to when results land and none to forget when the
 * field is cleared, and no completion-token suggestions (a Mac sidebar field's
 * device). A phone's field lives on the page.
 *
 * **Staleness.** Every edit takes a generation. A search that finishes under a
 * generation that is no longer current is dropped, so a slow answer to
 * "mil" never lands over the answer to "miles". Debounced, so a fast typist
 * does not queue a round of queries per keystroke.
 *
 * **Threading.** [setQuery] takes the text at once, from any thread, so a
 * field can echo it; the search itself runs on [scope], which is the
 * engine's dispatcher and never Main.
 */
class SearchModel(
    private val engine: KoanEngineInterface,
    private val mirror: EngineMirror,
    private val player: PlayerModel,
    private val navigator: Navigator,
    private val scope: CoroutineScope,
    private val debounceMs: Long = 160,
) {
    private val _query = MutableStateFlow("")
    private val _hasQuery = MutableStateFlow(false)
    private val _results = MutableStateFlow(SearchResults())
    private val lock = Any()
    private var generation = 0L
    private var task: Job? = null

    private val resolver = PlayableResolver(engine)

    /** The text in the field. */
    val query: StateFlow<String> = _query.asStateFlow()

    /**
     * Whether there is something to search for. Stored and written only when
     * it flips, so what asks (the empty state) is not a reader of every
     * keystroke.
     */
    val hasQuery: StateFlow<Boolean> = _hasQuery.asStateFlow()

    val results: StateFlow<SearchResults> = _results.asStateFlow()

    /**
     * A pick across the results, held through new queries (a pick can gather
     * from several searches) and ended with the page.
     */
    val selection = PlayableSelection { _results.value.playables() }

    /** Searching follows from the text changing. */
    fun setQuery(text: String) {
        val myGeneration: Long
        synchronized(lock) {
            if (text == _query.value) return
            _query.value = text
            task?.cancel()
            myGeneration = ++generation
            val has = text.isNotBlank()
            if (has != _hasQuery.value) _hasQuery.value = has
            if (!has) {
                task = null
                _results.value = SearchResults()
                return
            }
            // Only on the edge: written per keystroke, the page would re-run per keystroke for a flag that had not moved.
            if (!_results.value.searching) _results.value = _results.value.copy(searching = true)
            task = scope.launch { search(text.trim(), myGeneration) }
        }
    }

    /** Empty the field and the results, and cancel what is under way. */
    fun clear() = setQuery("")

    private suspend fun search(text: String, mine: Long) {
        delay(debounceMs)
        val shelf = try {
            engine.shelfSummary(ShelfKind.Search(text))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // A failed search finds nothing, as in Swift; it is not a page-wide error.
            null
        }
        synchronized(lock) {
            if (mine != generation) return
            _results.value = SearchResults(
                query = text,
                artists = shelf?.artists.orEmpty(),
                albums = shelf?.albums.orEmpty(),
                tracks = shelf?.tracks.orEmpty(),
                totals = shelf?.let(ShelfTotals::of),
                searching = false,
            )
        }
    }

    // MARK: - Acting on results and shelves

    /** The queue's version, for going to the Queue once a play has landed. */
    private val queueVersion: StateFlow<Long> =
        mirror.queueRows.map { it.version }.stateIn(scope, SharingStarted.Eagerly, mirror.queueRows.value.version)

    /** A selection for a page of its own (a shelf), over what it shows. */
    fun newSelection(grid: () -> List<Playable>) = PlayableSelection(grid)

    /** Play `ids` from `index`, keeping the rest behind it, and go to the queue once it has changed. */
    fun playTracks(ids: List<Long>, startingAt: Int) {
        if (ids.isEmpty()) return
        player.playNow(ids, startingAt)
        navigator.showQueueWhenReady(queueVersion)
    }

    /**
     * Play or queue a pick and put the page back. What the pick comes to is
     * asked of the engine here, off Main; the page has already returned.
     */
    fun commit(selection: PlayableSelection, play: Boolean) {
        val items = selection.finish()
        if (items.isEmpty()) return
        scope.launch {
            val ids = resolver.trackIds(items)
            if (play) playTracks(ids, 0) else player.enqueue(ids)
        }
    }
}

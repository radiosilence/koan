package cc.blit.koan.state

import cc.blit.koan.art.ArtworkSource
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.Playlist
import uniffi.koan_ffi.PlaylistEntry

/**
 * The playlists, and everything you can do to them. The port of
 * `PlaylistsModel.swift`, whose doc comments are the specification.
 *
 * Kept whole in memory: a playlist row is a name and two counts, and there are
 * tens of them, not thousands. Their *contents* are not; those are loaded a
 * playlist at a time, the same way an album's tracks are.
 *
 * Every mutation writes through the engine and nothing reloads here. Writing
 * playlist rows is a library change like any other, so the engine says so and
 * whatever is on screen asks again, the same path a sync from the server takes.
 * A reload written in per mutation is one that only covers the mutations
 * somebody remembered. The engine pushes to the server on its own, in the
 * background; nothing here waits on a network.
 *
 * Confined to [scope]'s dispatcher (the engine's, never Main); every public
 * method returns at once.
 */
class PlaylistsModel(
    private val engine: KoanEngineInterface,
    private val mirror: EngineMirror,
    private val scope: CoroutineScope,
) {
    private val _playlists = MutableStateFlow<List<Playlist>>(emptyList())
    private val _openId = MutableStateFlow<Long?>(null)
    private val _entries = MutableStateFlow<List<PlaylistEntry>>(emptyList())
    private val _loading = MutableStateFlow(false)
    private val _covers = MutableStateFlow<Map<Long, List<ArtworkSource>>>(emptyMap())
    private val _naming = MutableStateFlow<List<Long>?>(null)
    private val _report = MutableStateFlow<String?>(null)

    private var loadingList: Job? = null
    private var openStamp: Long? = null

    /** The `changedAt` each playlist's mosaic was built from: adding tracks changes a playlist's face. */
    private val coverStamp = HashMap<Long, String>()

    val playlists: StateFlow<List<Playlist>> = _playlists.asStateFlow()

    /** The playlist on screen. Only one is ever open. */
    val openId: StateFlow<Long?> = _openId.asStateFlow()

    /**
     * The open playlist's entries. Entries, not tracks: a playlist may hold the
     * same track twice, so a track id names neither a row nor a place. The entry
     * id does.
     */
    val entries: StateFlow<List<PlaylistEntry>> = _entries.asStateFlow()
    val isLoading: StateFlow<Boolean> = _loading.asStateFlow()

    /**
     * Up to four cover sources per playlist, for its 2 by 2 tile. Loaded lazily
     * and held, because a list draws every one of them on every update and
     * re-asking the database for them each time would be a query per row.
     */
    val covers: StateFlow<Map<Long, List<ArtworkSource>>> = _covers.asStateFlow()

    /**
     * Tracks waiting for a name, and the new playlist they will become. A
     * request rather than a dialog, because the dialog cannot live where it is
     * asked for: a menu is gone the instant you pick from it. One host presents
     * this, somewhere that outlives the gesture. Empty is a real request: a new
     * playlist with nothing in it yet.
     */
    val naming: StateFlow<List<Long>?> = _naming.asStateFlow()

    /** A failure the user should see; the shell shows it and calls [dismissReport]. */
    val report: StateFlow<String?> = _report.asStateFlow()

    fun dismissReport() {
        _report.value = null
    }

    fun beginNaming(trackIds: List<Long> = emptyList()) {
        _naming.value = trackIds
    }

    fun endNaming() {
        _naming.value = null
    }

    fun playlist(id: Long): Playlist? = _playlists.value.firstOrNull { it.id == id }

    // MARK: - Loading

    fun load() {
        // Cancelled on re-entry, so a slower earlier read can't land over a newer one.
        loadingList?.cancel()
        loadingList = scope.launch {
            val fetched = runCatchingEngine { engine.playlists() }.orEmpty()
            ensureActive()
            _playlists.value = fetched
            // A deleted playlist should stop holding memory.
            val live = fetched.mapTo(HashSet()) { it.id }
            _covers.update { it.filterKeys { id -> id in live } }
            coverStamp.keys.retainAll(live)
            for (playlist in fetched) {
                if (coverStamp[playlist.id] == playlist.changedAt) continue
                loadCovers(playlist.id)
                ensureActive()
                coverStamp[playlist.id] = playlist.changedAt
            }
        }
    }

    private suspend fun loadCovers(id: Long) {
        val ids = runCatchingEngine { engine.playlistCoverAlbumIds(id) }.orEmpty()
        _covers.update { it + (id to ids.map(ArtworkSource::Album)) }
    }

    /**
     * Read a playlist's rows *before* the navigator moves to it; the record's
     * shape (see `LibraryModel.prepareAlbum`). Rows and the mosaic over them land
     * as one value, so the page cannot draw its header above an empty list.
     * Asking again for the playlist already open is how a library change
     * reaches this page: free when nothing has moved under it, and a re-read that
     * replaces the rows in place rather than blanking them when something has.
     */
    suspend fun prepare(id: Long) {
        val stamp = mirror.libraryVersion.value
        if (_openId.value == id && openStamp == stamp) return
        _loading.value = true
        val (rows, covers) = coroutineScope {
            val rows = async { runCatchingEngine { engine.playlistTracks(id) }.orEmpty() }
            val covers = async { runCatchingEngine { engine.playlistCoverAlbumIds(id) }.orEmpty() }
            rows.await() to covers.await()
        }
        _openId.value = id
        openStamp = stamp
        _entries.value = rows
        _covers.update { it + (id to covers.map(ArtworkSource::Album)) }
        _loading.value = false
    }

    // MARK: - Mutations

    /** Create a playlist, optionally with tracks already in it. Hands back the new playlist so the caller can open it. */
    suspend fun create(name: String, trackIds: List<Long> = emptyList()): Playlist? {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return null
        return try {
            engine.createPlaylist(trimmed, trackIds)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            _report.value = "Couldn't create that playlist — ${e.message ?: e}"
            null
        }
    }

    fun rename(id: Long, name: String) {
        // Its file names it.
        if (playlist(id)?.fromFile == true) return
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return
        act { engine.renamePlaylist(id, trimmed) }
    }

    fun delete(id: Long) {
        if (_openId.value == id) {
            _openId.value = null
            openStamp = null
            _entries.value = emptyList()
        }
        act { engine.deletePlaylist(id) }
    }

    /** Whether the playlist takes edits to what it holds. A smart playlist's rules decide that. */
    fun fillable(id: Long): Boolean = playlist(id)?.readonly != true

    fun add(trackIds: List<Long>, to: Long) {
        if (trackIds.isEmpty() || !fillable(to)) return
        act { engine.addToPlaylist(to, trackIds) }
    }

    /** Landing where they were dropped rather than at the end; the engine adds and reorders in one go. */
    fun insert(trackIds: List<Long>, into: Long, at: Int) {
        if (trackIds.isEmpty() || !fillable(into)) return
        act { engine.insertIntoPlaylist(into, trackIds, at.toUInt()) }
    }

    /** Put the entries in this order. Ids survive, so the queue keeps knowing which row each of its items came from. */
    fun reorder(entryIds: List<Long>, inPlaylist: Long) {
        if (!fillable(inPlaylist)) return
        act { engine.reorderPlaylist(inPlaylist, entryIds) }
    }

    fun remove(entryIds: List<Long>, from: Long) {
        if (entryIds.isEmpty() || !fillable(from)) return
        act { engine.removeFromPlaylist(from, entryIds) }
    }

    /** Shuffle the playlist itself, permanently. Distinct from playing it shuffled, which leaves it alone. */
    fun shuffle(id: Long) {
        if (!fillable(id)) return
        act { engine.shufflePlaylist(id) }
    }

    /**
     * Put `moving` where `target` currently sits. Applied locally first, so the
     * row lands under the finger rather than a round trip later. Dropping a
     * playlist onto itself is a no-op rather than an error: it is the commonest
     * way to abandon a drag.
     */
    fun reorderPlaylists(moving: List<Long>, onto: Long) {
        scope.launch {
            val doomed = moving.toSet()
            if (onto in doomed) return@launch
            val order = _playlists.value.map { it.id }.filterNot { it in doomed }.toMutableList()
            val at = order.indexOf(onto)
            if (at < 0) return@launch
            order.addAll(at, moving)
            val byId = _playlists.value.associateBy { it.id }
            _playlists.value = order.mapNotNull { byId[it] }
            runCatchingEngine { engine.reorderPlaylists(order) }
        }
    }

    /** Remember how this playlist is looked at. Local to this device: a server has nowhere to put it. */
    fun setGrouped(grouped: Boolean, id: Long) {
        scope.launch {
            _playlists.update { list -> list.map { if (it.id == id) it.copy(grouped = grouped) else it } }
            runCatchingEngine { engine.setPlaylistGrouped(id, grouped) }
        }
    }

    /** A mutation, with the error reporting every one of them wants. */
    private fun act(body: suspend () -> Unit) {
        scope.launch {
            try {
                body()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _report.value = e.message ?: e.toString()
            }
        }
    }

    private suspend fun <T> runCatchingEngine(call: suspend () -> T): T? = try {
        call()
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        null
    }
}

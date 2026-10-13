package cc.blit.koan.state

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import uniffi.koan_ffi.Album
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.Track

/**
 * Something a page can play or queue: a track, a record, an artist. The port
 * of the Swift `Playable`, minus the playlist (no page here picks playlists).
 */
sealed interface Playable {
    val key: Key

    /**
     * What it is, without the row: artists, records and tracks share a page
     * and their ids collide, so a pick is held by kind and id.
     */
    data class Key(val kind: Kind, val id: Long)

    enum class Kind { Track, Album, Artist }

    data class OfTrack(val track: Track) : Playable {
        override val key = Key(Kind.Track, track.id)
    }

    data class OfAlbum(val album: Album) : Playable {
        override val key = Key(Kind.Album, album.id)
    }

    data class OfArtist(val id: Long, val name: String) : Playable {
        override val key = Key(Kind.Artist, id)
    }
}

/**
 * The tracks a pick comes to. A track is itself; a record or an artist asks
 * the engine, so this is only ever called from the engine's dispatcher. A
 * failed ask contributes nothing rather than failing the rest, as in Swift.
 */
class PlayableResolver(private val engine: KoanEngineInterface) {
    suspend fun trackIds(items: List<Playable>): List<Long> {
        val ids = ArrayList<Long>()
        for (item in items) {
            ids += when (item) {
                is Playable.OfTrack -> listOf(item.track.id)
                is Playable.OfAlbum -> ask { engine.trackIds(item.album.id, null) }
                is Playable.OfArtist -> ask { engine.trackIds(null, item.id) }
            }
        }
        return ids
    }

    private suspend fun ask(call: suspend () -> List<Long>): List<Long> = try {
        call()
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        emptyList()
    }
}

/** A pick as a page draws it: whether the mode is on, what is ticked and in what order. */
data class Pick(
    val active: Boolean = false,
    /** In the order things were ticked, not the order of the page. */
    val picked: List<Playable> = emptyList(),
) {
    private val members: Set<Playable.Key> = picked.mapTo(HashSet()) { it.key }

    operator fun contains(key: Playable.Key): Boolean = key in members
    val count: Int get() = picked.size
}

/**
 * Things picked out of a page to play or queue together: artists, records and
 * tracks in search results and shelves. The port of `PlayableSelection`.
 *
 * A mode rather than a tap: a tap already opens a record or a track's album,
 * and a long press starts picking. While picking, a tap ticks instead, and
 * playing or queueing the pick puts the page back to normal.
 *
 * Held in the order things were ticked: a pick can span several queries, and
 * the page has no order for something it is no longer showing. [grid] is what
 * the page shows now, in its order, which select-all reads.
 *
 * Plain state behind a flow; every method is safe from any thread. One per
 * page: a pick belongs to the page it was made in and ends when that page
 * leaves the screen.
 */
class PlayableSelection(private val grid: () -> List<Playable> = { emptyList() }) {
    private val _pick = MutableStateFlow(Pick())
    val pick: StateFlow<Pick> = _pick.asStateFlow()

    val isActive: Boolean get() = _pick.value.active

    /** Turn the mode on, with `playable` ticked if there is one. */
    fun begin(playable: Playable? = null) {
        _pick.update { start ->
            val on = start.copy(active = true)
            if (playable == null) on else toggled(on, playable)
        }
    }

    fun end() {
        _pick.update { if (it.active || it.picked.isNotEmpty()) Pick() else it }
    }

    /** A tap on an item while picking: flips just this one. */
    fun toggle(playable: Playable) {
        _pick.update { toggled(it, playable) }
    }

    /**
     * A tap on something that does something else outside the mode (opens,
     * plays). Whether the pick took it: a tick while picking.
     */
    fun take(playable: Playable): Boolean {
        if (!_pick.value.active) return false
        toggle(playable)
        return true
    }

    fun selectAll() {
        val all = grid()
        _pick.update { start ->
            val have = start.picked.mapTo(HashSet()) { it.key }
            Pick(true, start.picked + all.filter { have.add(it.key) })
        }
    }

    /** End the mode and hand back what was ticked. */
    fun finish(): List<Playable> {
        var items = emptyList<Playable>()
        _pick.update {
            items = it.picked
            Pick()
        }
        return items
    }

    private fun toggled(from: Pick, playable: Playable): Pick {
        val key = playable.key
        val picked = if (key in from) from.picked.filterNot { it.key == key } else from.picked + playable
        return from.copy(picked = picked)
    }
}

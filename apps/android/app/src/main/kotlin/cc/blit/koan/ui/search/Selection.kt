package cc.blit.koan.ui.search

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableStateSetOf
import androidx.compose.runtime.setValue
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.Track

/** Something a result stands for that can be played: an artist, a record or a track. */
sealed interface Playable {
    val key: String

    data class Artist(val id: Long, val name: String) : Playable {
        override val key get() = "artist:$id"
    }

    data class Record(val album: Album) : Playable {
        override val key get() = "album:${album.id}"
    }

    data class Song(val track: Track) : Playable {
        override val key get() = "track:${track.id}"
    }
}

/** The library tracks behind `this`, in running order. */
suspend fun Playable.trackIds(engine: KoanEngine): List<Long> = when (this) {
    is Playable.Song -> listOf(track.id)
    is Playable.Record -> engine.trackIds(album.id, null)
    is Playable.Artist -> engine.trackIds(null, id)
}

/**
 * Results picked to play or queue together, as `PlayableSelection` is on the
 * Apple apps. A mode: outside it a tap goes somewhere, inside it a tap ticks.
 *
 * Held in the order things were ticked rather than the page's, so a pick can
 * gather from several queries: search, tick, search again, tick more.
 */
class Selection {
    var active by mutableStateOf(false)
        private set

    /** What is ticked, in the order it was ticked. */
    val picked = mutableStateListOf<Playable>()
    private val members = mutableStateSetOf<String>()

    fun contains(playable: Playable) = playable.key in members

    fun begin(with: Playable? = null) {
        active = true
        with?.let(::toggle)
    }

    fun end() {
        active = false
        picked.clear()
        members.clear()
    }

    /** A tap on something that goes somewhere outside the mode. Whether the pick took it. */
    fun take(playable: Playable): Boolean {
        if (!active) return false
        toggle(playable)
        return true
    }

    fun toggle(playable: Playable) {
        if (members.remove(playable.key)) {
            picked.removeAll { it.key == playable.key }
        } else {
            members += playable.key
            picked += playable
        }
    }
}

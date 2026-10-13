package cc.blit.koan.ui.search

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableStateSetOf
import androidx.compose.runtime.setValue
import cc.blit.koan.ui.library.Playable

/** What a playable is, as one value, for telling picks apart. */
val Playable.key
    get() = when (this) {
        is Playable.OfTrack -> "track:${track.id}"
        is Playable.OfAlbum -> "album:${album.id}"
        is Playable.OfArtist -> "artist:$id"
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

    fun begin() {
        active = true
    }

    fun end() {
        active = false
        picked.clear()
        members.clear()
    }

    /** A tap on something that goes somewhere outside the mode. Whether the pick took it. */
    fun take(playable: Playable): Boolean {
        if (!active) return false
        if (members.remove(playable.key)) {
            picked.removeAll { it.key == playable.key }
        } else {
            members += playable.key
            picked += playable
        }
        return true
    }
}

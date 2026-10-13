package cc.blit.koan.ui.library

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.scale
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.KoanApp
import cc.blit.koan.R
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ui.koanApp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * What is favourited, as id sets every heart reads.
 *
 * A row's `isFavourite` is baked in when the row is read; the sets are the
 * live answer. A toggle sets them from what the engine returns, so the heart
 * answers the tap rather than the next read, and the whole set is read again
 * when the library moves, since a sync reconciles favourites with the server.
 */
class Favourites(private val mirror: EngineMirror) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private val _tracks = MutableStateFlow(emptySet<Long>())
    val tracks: StateFlow<Set<Long>> = _tracks.asStateFlow()

    private val _albums = MutableStateFlow(emptySet<Long>())
    val albums: StateFlow<Set<Long>> = _albums.asStateFlow()

    private val _artists = MutableStateFlow(emptySet<Long>())
    val artists: StateFlow<Set<Long>> = _artists.asStateFlow()

    init {
        scope.launch {
            mirror.library.collect {
                runCatching {
                    val engine = mirror.engine()
                    val tracks = async { engine.favouriteTrackIds().toSet() }
                    val albums = async { engine.favouriteAlbumIds().toSet() }
                    val artists = async { engine.favouriteArtistIds().toSet() }
                    _tracks.value = tracks.await()
                    _albums.value = albums.await()
                    _artists.value = artists.await()
                }
            }
        }
    }

    fun toggleTrack(id: Long) = toggle(_tracks, id) { toggleFavourite(id) }

    fun toggleAlbum(id: Long) = toggle(_albums, id) { toggleFavouriteAlbum(id) }

    fun toggleArtist(id: Long) = toggle(_artists, id) { toggleFavouriteArtist(id) }

    private fun toggle(set: MutableStateFlow<Set<Long>>, id: Long, call: suspend KoanEngine.() -> Boolean) {
        scope.launch {
            val on = runCatching { mirror.engine().call() }.getOrNull() ?: return@launch
            set.update { if (on) it + id else it - id }
        }
    }

    companion object {
        private var held: Favourites? = null

        /** One per process, beside the engine it reads. */
        fun of(app: KoanApp): Favourites = synchronized(this) {
            held ?: Favourites(app.mirror).also { held = it }
        }
    }
}

@Composable
fun favourites() = Favourites.of(koanApp())

/**
 * The heart, wherever something can be favourited: filled in the error role
 * when on, outlined and muted when off. A row that would otherwise show a
 * column of empty hearts passes `showing = false`, and then only a
 * favourite's heart is drawn.
 */
@Composable
fun FavouriteButton(
    isOn: Boolean,
    onToggle: () -> Unit,
    modifier: Modifier = Modifier,
    showing: Boolean = true,
    size: Dp = 20.dp,
) {
    if (!isOn && !showing) return
    val haptics = LocalHapticFeedback.current
    val pop by animateFloatAsState(if (isOn) 1f else 0.9f, spring(dampingRatio = 0.4f), label = "heart")
    IconButton(
        onClick = {
            if (!isOn) haptics.performHapticFeedback(HapticFeedbackType.ToggleOn)
            onToggle()
        },
        modifier = modifier,
    ) {
        Icon(
            painterResource(if (isOn) R.drawable.ic_heart_filled else R.drawable.ic_heart),
            contentDescription = if (isOn) "Remove favourite" else "Favourite",
            tint = if (isOn) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.size(size).scale(pop),
        )
    }
}

// Each heart reads its set itself, so a toggle recomposes the hearts and not
// the rows that hold them.

@Composable
fun TrackHeart(trackId: Long, modifier: Modifier = Modifier, showing: Boolean = true) {
    val store = favourites()
    val ids by store.tracks.collectAsStateWithLifecycle()
    FavouriteButton(trackId in ids, { store.toggleTrack(trackId) }, modifier, showing)
}

@Composable
fun AlbumHeart(albumId: Long, modifier: Modifier = Modifier, showing: Boolean = true) {
    val store = favourites()
    val ids by store.albums.collectAsStateWithLifecycle()
    FavouriteButton(albumId in ids, { store.toggleAlbum(albumId) }, modifier, showing)
}

@Composable
fun ArtistHeart(artistId: Long, modifier: Modifier = Modifier, showing: Boolean = true) {
    val store = favourites()
    val ids by store.artists.collectAsStateWithLifecycle()
    FavouriteButton(artistId in ids, { store.toggleArtist(artistId) }, modifier, showing)
}

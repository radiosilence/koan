package cc.blit.koan.ui.library

import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.KoanApp
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanLabel
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
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
 * The heart, wherever something can be favourited: filled in `bad` when on,
 * an outline in `muted` when off. A row that would otherwise show a column of
 * empty hearts passes `showing = false`, and then only a favourite's heart is
 * drawn. A page's header passes `labelled`: the heart with its word beside
 * it, "favourite", or "favourited" in `bad` once on, as the iPhone's header has it.
 */
@Composable
fun FavouriteButton(
    isOn: Boolean,
    onToggle: () -> Unit,
    modifier: Modifier = Modifier,
    showing: Boolean = true,
    labelled: Boolean = false,
) {
    if (!isOn && !showing) return
    val haptics = LocalHapticFeedback.current
    val toggle = {
        if (!isOn) haptics.performHapticFeedback(HapticFeedbackType.ToggleOn)
        onToggle()
    }
    val icon = if (isOn) KoanIcons.favourited else KoanIcons.favourite
    if (labelled) {
        KoanButton(toggle, modifier, kind = KoanButtonKind.Compact) {
            val color = KoanTheme.color(if (isOn) KoanTone.Bad else KoanTone.Ink)
            CompositionLocalProvider(LocalContentColor provides color, LocalTextStyle provides LocalTextStyle.current.copy(color = color)) {
                KoanLabel(if (isOn) "Favourited" else "Favourite", icon)
            }
        }
        return
    }
    KoanButton(toggle, modifier, kind = KoanButtonKind.Icon) {
        KoanIcon(icon, if (isOn) "Remove favourite" else "Favourite", color = KoanTheme.color(if (isOn) KoanTone.Bad else KoanTone.Muted))
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
fun AlbumHeart(albumId: Long, modifier: Modifier = Modifier, showing: Boolean = true, labelled: Boolean = false) {
    val store = favourites()
    val ids by store.albums.collectAsStateWithLifecycle()
    FavouriteButton(albumId in ids, { store.toggleAlbum(albumId) }, modifier, showing, labelled)
}

@Composable
fun ArtistHeart(artistId: Long, modifier: Modifier = Modifier, showing: Boolean = true, labelled: Boolean = false) {
    val store = favourites()
    val ids by store.artists.collectAsStateWithLifecycle()
    FavouriteButton(artistId in ids, { store.toggleArtist(artistId) }, modifier, showing, labelled)
}

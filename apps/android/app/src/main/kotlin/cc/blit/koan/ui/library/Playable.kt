package cc.blit.koan.ui.library

import androidx.compose.foundation.layout.Box
import androidx.compose.material3.DropdownMenu
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.KoanApp
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Navigator
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.playlists.AddToPlaylistSheet
import cc.blit.koan.ui.playlists.MenuItem
import cc.blit.koan.ui.theme.KoanDivider
import cc.blit.koan.ui.theme.KoanIcons

/**
 * Anything that can go in the queue. Tracks, records and artists differ only
 * in how many tracks they come to, so they share one menu; the tracks are
 * read when an action is chosen, since an artist can be thousands of them.
 */
sealed interface Playable {
    val name: String

    data class OfTrack(val track: Track) : Playable {
        override val name get() = track.title
    }

    data class OfAlbum(val album: Album) : Playable {
        override val name get() = album.title
    }

    data class OfArtist(val id: Long, override val name: String) : Playable

    suspend fun trackIds(engine: KoanEngine): List<Long> = when (this) {
        is OfTrack -> listOf(track.id)
        is OfAlbum -> engine.trackIds(album.id, null)
        is OfArtist -> engine.trackIds(null, id)
    }
}

/** Replace the queue with `playable`, in order or shuffled from a track at random. */
fun KoanApp.play(playable: Playable, shuffled: Boolean = false) = mirror.launch {
    val ids = playable.trackIds(this)
    if (ids.isEmpty()) return@launch
    replaceQueue(ids, (if (shuffled) ids.indices.random() else 0).toUInt(), shuffled)
}

/**
 * The long-press menu for a playable thing, anchored where it is placed.
 * Play, Play Next, Add to Queue and Shuffle; the heart; fetching or removing
 * downloads; and the record and artist it belongs to.
 */
@Composable
fun PlayableMenu(playable: Playable, expanded: Boolean, onDismiss: () -> Unit) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val context = LocalContext.current
    val signedIn = app.mirror.connection.collectAsStateWithLifecycle().value != null
    val store = favourites()
    val favouriteTracks by store.tracks.collectAsStateWithLifecycle()
    val favouriteAlbums by store.albums.collectAsStateWithLifecycle()
    val favouriteArtists by store.artists.collectAsStateWithLifecycle()
    fun act(block: suspend KoanEngine.(List<Long>) -> Unit) {
        onDismiss()
        app.mirror.launch {
            val ids = playable.trackIds(this)
            if (ids.isNotEmpty()) block(ids)
        }
    }
    var adding by remember { mutableStateOf(false) }
    if (adding) AddToPlaylistSheet({ playable.trackIds(this) }) { adding = false }
    Box {
        DropdownMenu(expanded = expanded, onDismissRequest = onDismiss) {
            MenuItem("Play", KoanIcons.play) { onDismiss(); app.play(playable) }
            MenuItem("Play Next", KoanIcons.playNext) { act { ids -> app.player.playNext(ids) } }
            MenuItem("Add to Queue", KoanIcons.queue) { act { ids -> addToQueue(ids) } }
            if (playable !is Playable.OfTrack) MenuItem("Shuffle", KoanIcons.shuffle) { onDismiss(); app.play(playable, shuffled = true) }
            MenuItem("Add to Playlist…", KoanIcons.playlist) { onDismiss(); adding = true }
            KoanDivider()
            when (playable) {
                is Playable.OfTrack -> {
                    val on = playable.track.id in favouriteTracks
                    MenuItem(if (on) "Remove Favourite" else "Favourite Track", if (on) KoanIcons.favourited else KoanIcons.favourite) { onDismiss(); store.toggleTrack(playable.track.id) }
                }
                is Playable.OfAlbum -> {
                    val on = playable.album.id in favouriteAlbums
                    MenuItem(if (on) "Remove Favourite" else "Favourite Album", if (on) KoanIcons.favourited else KoanIcons.favourite) { onDismiss(); store.toggleAlbum(playable.album.id) }
                }
                is Playable.OfArtist -> {
                    val on = playable.id in favouriteArtists
                    MenuItem(if (on) "Remove Favourite" else "Favourite Artist", if (on) KoanIcons.favourited else KoanIcons.favourite) { onDismiss(); store.toggleArtist(playable.id) }
                }
            }
            if (signedIn) MenuItem("Share", KoanIcons.share) { onDismiss(); app.share(context, playable) }
            // A track knows whether its file is here, so it is offered the one
            // verb that does something; a record or an artist is usually part
            // of the way there, and both apply.
            if (playable is Playable.OfTrack) {
                if (playable.track.onDisk) {
                    MenuItem("Remove Download", KoanIcons.clear) { act { ids -> clearDownloads(ids) } }
                } else {
                    MenuItem("Download", KoanIcons.downloads) { act { ids -> downloadToCache(ids) } }
                }
            } else {
                MenuItem("Download All", KoanIcons.downloads) { act { ids -> downloadToCache(ids) } }
                MenuItem("Remove Downloads", KoanIcons.clear) { act { ids -> clearDownloads(ids) } }
            }
            val albumId = when (playable) {
                is Playable.OfTrack -> playable.track.albumId
                is Playable.OfAlbum -> playable.album.id
                is Playable.OfArtist -> null
            }
            val artistId = when (playable) {
                is Playable.OfTrack -> playable.track.artistId
                is Playable.OfAlbum -> playable.album.artistId
                is Playable.OfArtist -> null
            }
            if (albumId != null || artistId != null) KoanDivider()
            if (albumId != null && playable !is Playable.OfAlbum) {
                MenuItem("Go to Album", KoanIcons.album) { onDismiss(); nav.push(AlbumRoute(albumId, (playable as? Playable.OfTrack)?.track?.id)) }
            }
            if (artistId != null) MenuItem("Go to Artist", KoanIcons.artist) { onDismiss(); nav.push(ArtistRoute(artistId)) }
        }
    }
}

/** Play `trackIds` from `index`, then show the queue, as a list gathered from all over does. */
fun KoanApp.playFrom(trackIds: List<Long>, index: Int, nav: Navigator) {
    player.playTracks(trackIds, index)
    nav.select(Tab.Queue)
}

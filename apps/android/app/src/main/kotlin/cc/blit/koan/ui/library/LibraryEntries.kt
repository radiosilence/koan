package cc.blit.koan.ui.library

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.downloads.DownloadsRoute
import cc.blit.koan.ui.history.HistoryRoute
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.playlists.PlaylistsRoute
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanNavRow
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.LocalKoanTokens

fun EntryProviderScope<NavKey>.libraryEntries() {
    entry<Tab.Library> { LibraryPage() }
    entry<AlbumsRoute> { key -> AlbumsPage(key.shelf) }
    entry<ArtistsRoute> { key -> ArtistsPage(key.shelf) }
    entry<TracksRoute> { key -> TracksPage(key.shelf) }
    entry<ShelfRoute> { key -> ShelfPage(key.shelf) }
    entry<AlbumRoute> { key -> AlbumPage(key.albumId, key.track) }
    entry<ArtistRoute> { key -> ArtistPage(key.artistId) }
}

/**
 * Everything browsed, behind one tab, as the phone has it: the albums and
 * artists, the shelves (favourites, what was played lately, what is on this
 * device), playlists, history and downloads. No Tracks: the whole library as
 * one list is tens of thousands of rows to draw and play from at once, so the
 * track browser is reached filtered, from a shelf's.
 */
@Composable
private fun LibraryPage() {
    val nav = LocalNavigator.current
    Column {
        PageBar("Library", back = false)
        LazyColumn {
            item { LibraryStatus() }
            item { KoanNavRow("Albums", KoanIcons.album, false, { nav.push(AlbumsRoute()) }) }
            item { KoanNavRow("Artists", KoanIcons.artist, false, { nav.push(ArtistsRoute()) }) }
            item { KoanNavRow("Favourites", KoanIcons.favourite, false, { nav.push(ShelfRoute(Shelf.Favourites)) }) }
            item { KoanNavRow("Playlists", KoanIcons.playlist, false, { nav.push(PlaylistsRoute()) }) }
            item { KoanNavRow("Recently Played", KoanIcons.recentlyPlayed, false, { nav.push(ShelfRoute(Shelf.Recent)) }) }
            item { KoanNavRow("Downloaded", KoanIcons.onDevice, false, { nav.push(ShelfRoute(Shelf.Downloaded)) }) }
            item { KoanNavRow("History", KoanIcons.history, false, { nav.push(HistoryRoute()) }) }
            item { KoanNavRow("Downloads", KoanIcons.downloads, false, { nav.push(DownloadsRoute()) }) }
        }
    }
}

/**
 * Why the library shows less than it holds: the server refused this device's
 * sign-in, or koan is offline, by hand, with the way back, or because the
 * server is out of reach, which lifts by itself.
 */
@Composable
private fun LibraryStatus() {
    val app = koanApp()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val held = connection ?: return
    val (title, detail) = when {
        held.signInRefused -> "Your server refused this device's sign-in" to "Sign in again in Settings."
        held.offline -> (if (held.offlineManual) "Offline mode is on" else "Can't reach your server") to "Showing what is on this device."
        else -> return
    }
    Column(Modifier.padding(horizontal = LocalKoanTokens.current.gutter, vertical = KoanSpace.s)) {
        KoanText(title, tone = if (held.signInRefused) KoanTone.Bad else KoanTone.Ink, appText = true)
        KoanText(detail, role = KoanType.Meta, tone = KoanTone.Muted, appText = true)
        if (!held.signInRefused && held.offlineManual) {
            KoanButton("Go Online", { app.mirror.launch { setOffline(false) } }, kind = KoanButtonKind.Link)
        }
    }
}

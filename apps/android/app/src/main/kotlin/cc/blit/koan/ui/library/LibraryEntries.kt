package cc.blit.koan.ui.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.R
import cc.blit.koan.ui.downloads.DownloadsRoute
import cc.blit.koan.ui.history.HistoryRoute
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.playlists.PlaylistsRoute

fun EntryProviderScope<NavKey>.libraryEntries() {
    entry<Tab.Library> { LibraryPage() }
    entry<AlbumsRoute> { key -> AlbumsPage(key.shelf) }
    entry<ArtistsRoute> { key -> ArtistsPage(key.shelf) }
    entry<TracksRoute> { key -> TracksPage(key.shelf) }
    entry<ShelfRoute> { key -> ShelfPage(key.shelf) }
    entry<AlbumRoute> { key -> AlbumPage(key.albumId) }
    entry<ArtistRoute> { key -> ArtistPage(key.artistId) }
}

/**
 * Everything browsed, behind one tab, as the phone has it: the albums and
 * artists, the shelves (favourites, what was played lately, what is on this
 * device), playlists and downloads. No Tracks: the whole library as one list
 * is tens of thousands of rows to draw and play from at once, so the track
 * browser is reached filtered, from a shelf's.
 */
@Composable
private fun LibraryPage() {
    val nav = LocalNavigator.current
    Column {
        PageBar("Library", back = false)
        LazyColumn {
            item { LibraryStatus() }
            item { Link("Albums", R.drawable.ic_album) { nav.push(AlbumsRoute()) } }
            item { Link("Artists", R.drawable.ic_artist) { nav.push(ArtistsRoute()) } }
            item { Link("Favourites", R.drawable.ic_heart) { nav.push(ShelfRoute(Shelf.Favourites)) } }
            item { Link("Playlists", R.drawable.ic_playlist) { nav.push(PlaylistsRoute()) } }
            item { Link("Recently Played", R.drawable.ic_clock) { nav.push(ShelfRoute(Shelf.Recent)) } }
            item { Link("Downloaded", R.drawable.ic_drive) { nav.push(ShelfRoute(Shelf.Downloaded)) } }
            item { Link("History", R.drawable.ic_clock) { nav.push(HistoryRoute()) } }
            item { Link("Downloads", R.drawable.ic_download) { nav.push(DownloadsRoute()) } }
        }
    }
}

@Composable
private fun Link(title: String, icon: Int, onClick: () -> Unit) {
    ListItem(
        headlineContent = { Text(title) },
        leadingContent = { Icon(painterResource(icon), null) },
        trailingContent = { Icon(painterResource(R.drawable.ic_chevron_right), null) },
        modifier = Modifier.clickable(onClick = onClick),
    )
}

/**
 * Why the library shows less than it holds: koan is offline, by hand, with
 * the way back, or because the server is out of reach, which lifts by itself.
 */
@Composable
private fun LibraryStatus() {
    val app = koanApp()
    val connection by app.mirror.connection.collectAsStateWithLifecycle()
    val held = connection ?: return
    if (held.signInRefused) {
        return ListItem(
            headlineContent = { Text("Your server refused this device's sign-in") },
            supportingContent = { Text("Sign in again in Settings.", color = MaterialTheme.colorScheme.onSurfaceVariant) },
        )
    }
    if (!held.offline) return
    ListItem(
        headlineContent = { Text(if (held.offlineManual) "Offline mode is on" else "Can't reach your server") },
        supportingContent = {
            Text("Showing what is on this device.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        },
        trailingContent = {
            if (held.offlineManual) TextButton(onClick = { app.mirror.launch { setOffline(false) } }) { Text("Go Online") }
        },
    )
}

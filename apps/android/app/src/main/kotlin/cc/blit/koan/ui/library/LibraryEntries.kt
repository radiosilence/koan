package cc.blit.koan.ui.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.ListItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.art.Art
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.downloads.DownloadedRoute
import cc.blit.koan.ui.downloads.DownloadsRoute
import cc.blit.koan.ui.history.HistoryRoute
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.playlists.PlaylistsRoute
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage

fun EntryProviderScope<NavKey>.libraryEntries() {
    entry<Tab.Library> { LibraryPage() }
    entry<AlbumRoute> { key -> AlbumPage(key.albumId) }
    entry<ArtistRoute> { StubPage("Artist") }
}

@Composable
private fun LibraryPage() {
    val app = koanApp()
    val model = viewModel { LibraryModel(app.mirror) }
    val albums by model.albums.collectAsStateWithLifecycle()
    val nav = LocalNavigator.current
    val thumb = LocalKoanTokens.current.thumb
    LazyColumn {
        item { ListItem(headlineContent = { Text("Playlists") }, modifier = Modifier.clickable { nav.push(PlaylistsRoute()) }) }
        item { ListItem(headlineContent = { Text("Downloaded") }, modifier = Modifier.clickable { nav.push(DownloadedRoute()) }) }
        item { ListItem(headlineContent = { Text("History") }, modifier = Modifier.clickable { nav.push(HistoryRoute()) }) }
        item { ListItem(headlineContent = { Text("Downloads") }, modifier = Modifier.clickable { nav.push(DownloadsRoute()) }) }
        items(albums, key = { it.id }) { album ->
            ListItem(
                headlineContent = { Text(album.title) },
                supportingContent = { Text(album.artistName) },
                leadingContent = { AsyncImage(Art.album(album.id), null, Modifier.size(thumb)) },
                modifier = Modifier.clickable { nav.push(AlbumRoute(album.id)) },
            )
        }
    }
}

@Composable
private fun AlbumPage(albumId: Long) {
    val app = koanApp()
    val model = viewModel { AlbumModel(app.mirror, albumId) }
    val page by model.page.collectAsStateWithLifecycle()
    val tracks = page?.tracks.orEmpty()
    LazyColumn {
        itemsIndexed(tracks, key = { _, t -> t.id }) { index, track ->
            ListItem(
                headlineContent = { Text(track.title) },
                supportingContent = { Text(track.artistName) },
                leadingContent = { Text(track.trackNumber?.toString() ?: "") },
                modifier = Modifier.clickable { app.player.playTracks(tracks.map { it.id }, index) },
            )
        }
    }
}

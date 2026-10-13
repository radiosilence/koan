package cc.blit.koan.ui.playlists

import android.widget.Toast
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.LocalKoanTokens
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * "Add to playlist", as a sheet: every playlist that takes tracks, and a new
 * one. Show it while something is being added and dismiss it on `onDismiss`:
 *
 * ```
 * var adding by remember { mutableStateOf(false) }
 * if (adding) AddToPlaylistSheet({ trackIds(albumId, null) }) { adding = false }
 * ```
 *
 * `trackIds` resolves what is being added against the engine, off the main
 * thread, once a playlist is chosen: an album or an artist is a query, and
 * nothing is asked if the sheet is dismissed. `onAdded` follows `onDismiss`
 * when a playlist was chosen or made.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddToPlaylistSheet(trackIds: suspend KoanEngine.() -> List<Long>, onAdded: () -> Unit = {}, onDismiss: () -> Unit) {
    val app = koanApp()
    val edits = remember { PlaylistEdits(app.mirror) }
    val playlists by produceState<List<Playlist>?>(null) {
        value = runCatching { app.mirror.engine().playlists() }.getOrDefault(emptyList()).filter { it.fillable }
    }
    var naming by remember { mutableStateOf(false) }
    val thumb = LocalKoanTokens.current.thumb

    // Said after the sheet has gone, so with the application's context.
    suspend fun added(count: UInt, name: String) = withContext(Dispatchers.Main) {
        Toast.makeText(app, "Added ${trackCount(count)} to $name", Toast.LENGTH_SHORT).show()
    }

    if (naming) {
        NameDialog("New Playlist", "Create", onDismiss = onDismiss) { name ->
            app.mirror.launch {
                val ids = trackIds()
                val made = edits.create(name, ids) ?: return@launch
                added(ids.size.toUInt(), made.name)
            }
            onDismiss()
            onAdded()
        }
        return
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        LazyColumn(Modifier.navigationBarsPadding()) {
            item { KoanSectionHeader("Add to Playlist", Modifier.padding(horizontal = LocalKoanTokens.current.gutter)) }
            item {
                CollectionRow(
                    "New Playlist…",
                    null,
                    appText = true,
                    onClick = { naming = true },
                    leading = { KoanIcon(KoanIcons.add, null) },
                )
            }
            items(playlists.orEmpty(), key = { it.id }) { playlist ->
                val covers by produceState(emptyList<Long>(), playlist.id, playlist.changedAt) {
                    value = runCatching { app.mirror.engine().playlistCoverAlbumIds(playlist.id) }.getOrDefault(emptyList())
                }
                CollectionRow(
                    playlist.name,
                    trackCount(playlist.trackCount),
                    onClick = {
                        app.mirror.launch { added(addToPlaylist(playlist.id, trackIds()), playlist.name) }
                        onDismiss()
                        onAdded()
                    },
                    leading = { PlaylistMosaic(covers, thumb) },
                )
            }
        }
    }
}

fun trackCount(count: UInt) = if (count == 1u) "1 track" else "$count tracks"

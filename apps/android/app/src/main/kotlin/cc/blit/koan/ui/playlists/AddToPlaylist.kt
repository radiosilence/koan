package cc.blit.koan.ui.playlists

import android.widget.Toast
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import cc.blit.koan.R
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ui.koanApp
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
 * nothing is asked if the sheet is dismissed.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddToPlaylistSheet(trackIds: suspend KoanEngine.() -> List<Long>, onDismiss: () -> Unit) {
    val app = koanApp()
    val edits = remember { PlaylistEdits(app.mirror) }
    val playlists by produceState<List<Playlist>?>(null) {
        value = runCatching { app.mirror.engine().playlists() }.getOrDefault(emptyList()).filter { it.fillable }
    }
    var naming by remember { mutableStateOf(false) }
    val thumb = LocalKoanTokens.current.thumb

    // After the sheet has gone, so said with the application's context.
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
        }
        return
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        LazyColumn(Modifier.navigationBarsPadding()) {
            item {
                ListItem(
                    headlineContent = { Text("New Playlist…") },
                    leadingContent = { Icon(painterResource(R.drawable.koan_plus), null) },
                    modifier = Modifier.clickable { naming = true },
                )
            }
            if (!playlists.isNullOrEmpty()) item { HorizontalDivider() }
            items(playlists.orEmpty(), key = { it.id }) { playlist ->
                val covers by produceState(emptyList<Long>(), playlist.id, playlist.changedAt) {
                    value = runCatching { app.mirror.engine().playlistCoverAlbumIds(playlist.id) }.getOrDefault(emptyList())
                }
                ListItem(
                    headlineContent = { Text(playlist.name) },
                    supportingContent = { Text(trackCount(playlist.trackCount)) },
                    leadingContent = { PlaylistMosaic(covers, thumb) },
                    modifier = Modifier.clickable {
                        app.mirror.launch {
                            val count = addToPlaylist(playlist.id, trackIds())
                            added(count, playlist.name)
                        }
                        onDismiss()
                    },
                )
            }
        }
    }
}

fun trackCount(count: UInt) = if (count == 1u) "1 track" else "$count tracks"

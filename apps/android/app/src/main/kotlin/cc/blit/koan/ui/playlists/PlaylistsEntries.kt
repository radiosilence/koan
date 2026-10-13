package cc.blit.koan.ui.playlists

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.R
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.LocalKoanTokens
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable

@Serializable
data class PlaylistsRoute(val nonce: String = nonce()) : NavKey

@Serializable
data class PlaylistRoute(val playlistId: Long, val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.playlistsEntries() {
    entry<PlaylistsRoute> { PlaylistsPage() }
    entry<PlaylistRoute> { key -> PlaylistPage(key.playlistId) }
}

/** The playlists, as a page of the library; each leads to its `PlaylistPage`. */
@Composable
private fun PlaylistsPage() {
    val app = koanApp()
    val model = viewModel { PlaylistsModel(app.mirror) }
    val playlists by model.playlists.collectAsStateWithLifecycle()
    val edits = remember { PlaylistEdits(app.mirror) }
    val nav = LocalNavigator.current
    val scope = rememberCoroutineScope()
    var naming by remember { mutableStateOf(false) }

    Column {
        PageBar("Playlists") {
            IconButton(onClick = { naming = true }) { Icon(painterResource(R.drawable.koan_plus), "New Playlist") }
        }
        val shown = playlists
        when {
            shown == null -> Unit
            shown.isEmpty() -> StubPage("No playlists")
            else -> LazyColumn {
                items(shown, key = { it.id }) { playlist ->
                    PlaylistListRow(playlist, edits) { nav.push(PlaylistRoute(playlist.id)) }
                }
            }
        }
    }

    if (naming) {
        NameDialog("New Playlist", "Create", onDismiss = { naming = false }) { name ->
            naming = false
            scope.launch { edits.create(name)?.let { nav.push(PlaylistRoute(it.id)) } }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun PlaylistListRow(playlist: Playlist, edits: PlaylistEdits, open: () -> Unit) {
    val app = koanApp()
    val thumb = LocalKoanTokens.current.thumb
    // Keyed on `changedAt`: adding tracks changes a playlist's face.
    val covers by produceState(emptyList<Long>(), playlist.id, playlist.changedAt) {
        value = runCatching { app.mirror.engine().playlistCoverAlbumIds(playlist.id) }.getOrDefault(emptyList())
    }
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }

    Box {
        ListItem(
            headlineContent = { Text(playlist.name) },
            supportingContent = { Text(playlist.summary(), color = MaterialTheme.colorScheme.onSurfaceVariant) },
            leadingContent = { PlaylistMosaic(covers, thumb) },
            modifier = Modifier.combinedClickable(onClick = open, onLongClick = { menu = true }),
        )
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            DropdownMenuItem(text = { Text("Play") }, onClick = { menu = false; edits.play(playlist.id) })
            DropdownMenuItem(text = { Text("Shuffle") }, onClick = { menu = false; edits.play(playlist.id, shuffled = true) })
            if (!playlist.fromFile) {
                DropdownMenuItem(text = { Text("Rename…") }, onClick = { menu = false; renaming = true })
            }
            DropdownMenuItem(text = { Text("Delete Playlist") }, onClick = { menu = false; deleting = true })
        }
    }
    if (renaming) {
        NameDialog("Rename Playlist", "Rename", playlist.name, onDismiss = { renaming = false }) {
            renaming = false
            edits.rename(playlist, it)
        }
    }
    if (deleting) {
        ConfirmDialog(
            "Delete “${playlist.name}”?",
            if (playlist.remoteId != null) "It is deleted on your server too." else "This cannot be undone.",
            "Delete",
            onDismiss = { deleting = false },
        ) { edits.delete(playlist.id) }
    }
}

/**
 * "12 tracks · Smart", saying what the list needs to: how much is in it, and
 * whether it is one to edit.
 */
fun Playlist.summary(): String {
    val parts = mutableListOf(trackCount(trackCount))
    when {
        smart -> parts += "Smart"
        fromFile -> parts += "From a file"
        readonly -> parts += "Read-only"
    }
    return parts.joinToString(" · ")
}

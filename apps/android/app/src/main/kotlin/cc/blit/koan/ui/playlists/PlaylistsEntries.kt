package cc.blit.koan.ui.playlists

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.DropdownMenu
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanUnavailable
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
            KoanButton("New Playlist", { naming = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.add)
        }
        val shown = playlists
        when {
            shown == null -> Unit
            shown.isEmpty() -> KoanUnavailable("No playlists", KoanIcons.playlist, "Made here or on your server, they show up in both.")
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
        CollectionRow(
            playlist.name,
            playlist.summary(),
            onClick = open,
            onLongClick = { menu = true },
            leading = { PlaylistMosaic(covers, thumb) },
        )
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            MenuItem("Play", KoanIcons.play) { menu = false; edits.play(playlist.id) }
            MenuItem("Shuffle", KoanIcons.shuffle) { menu = false; edits.play(playlist.id, shuffled = true) }
            if (!playlist.fromFile) MenuItem("Rename…") { menu = false; renaming = true }
            MenuItem("Delete Playlist", KoanIcons.clear, destructive = true) { menu = false; deleting = true }
        }
    }
    if (renaming) {
        NameDialog("Rename Playlist", "Rename", playlist.name, onDismiss = { renaming = false }) {
            renaming = false
            edits.rename(playlist, it)
        }
    }
    if (deleting) DeletePlaylistDialog(playlist, onDismiss = { deleting = false }) { edits.delete(playlist.id) }
}

@Composable
fun DeletePlaylistDialog(playlist: Playlist, onDismiss: () -> Unit, onDelete: () -> Unit) {
    ConfirmDialog(
        "Delete Playlist?",
        "“${playlist.name}” is deleted" + if (playlist.remoteId != null) " here and on your server." else ". This cannot be undone.",
        "Delete",
        onDismiss = onDismiss,
        onConfirm = onDelete,
    )
}

/** "12 tracks · smart", saying what the list needs to: how much is in it, and whether it is one to edit. */
fun Playlist.summary(): String {
    val parts = mutableListOf(trackCount(trackCount))
    when {
        smart -> parts += "smart"
        fromFile -> parts += "from a file"
        readonly -> parts += "read-only"
    }
    return parts.joinToString(" · ")
}

package cc.blit.koan.ui.playlists

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.engine.attempt
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.PageBar
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.shown
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.value
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable
import sh.calvin.reorderable.ReorderableItem
import sh.calvin.reorderable.rememberReorderableLazyListState

@Serializable
data class PlaylistsRoute(val nonce: String = nonce()) : NavKey

@Serializable
data class PlaylistRoute(val playlistId: Long, val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.playlistsEntries() {
    entry<PlaylistsRoute> { PlaylistsPage() }
    entry<PlaylistRoute> { key -> PlaylistPage(key.playlistId) }
}

/**
 * The playlists, as a page of the library; each leads to its `PlaylistPage`.
 * Edit puts a handle on each row to drag it by.
 */
@Composable
private fun PlaylistsPage() {
    val app = koanApp()
    val model = viewModel { PlaylistsModel(app.mirror) }
    val read by model.playlists.state.collectAsStateWithLifecycle()
    val playlists = read.value
    val edits = remember { PlaylistEdits(app.mirror) }
    val nav = LocalNavigator.current
    val scope = rememberCoroutineScope()
    var naming by remember { mutableStateOf(false) }
    var editing by rememberSaveable { mutableStateOf(false) }

    Column {
        PageBar("Playlists") {
            if (editing || playlists.orEmpty().size > 1) {
                KoanButton(if (editing) "Done" else "Edit", { editing = !editing }, kind = KoanButtonKind.Text)
            }
            KoanButton("New Playlist", { naming = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.add)
        }
        val shown = read.shown("your playlists", model.playlists::retry)
        when {
            shown == null -> Unit
            shown.isEmpty() -> KoanUnavailable("No playlists", KoanIcons.playlist, "Made here or on your server, they show up in both.")
            else -> PlaylistsColumn(shown, editing, edits) { nav.push(PlaylistRoute(it)) }
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
private fun PlaylistsColumn(playlists: List<Playlist>, editing: Boolean, edits: PlaylistEdits, open: (Long) -> Unit) {
    val listState = rememberLazyListState()
    val latest by rememberUpdatedState(playlists)
    var order by remember { mutableStateOf(playlists) }
    var dragging by remember { mutableStateOf(false) }
    // The engine's order, except under a drag: a sync landing then must not
    // move rows out from under the finger. The drop is written and comes back.
    LaunchedEffect(playlists) { if (!dragging) order = playlists }
    val haptics = LocalHapticFeedback.current
    val reorder = rememberReorderableLazyListState(listState) { from, to ->
        val a = order.indexOfFirst { it.id == from.key }
        val b = order.indexOfFirst { it.id == to.key }
        if (a >= 0 && b >= 0) {
            order = order.toMutableList().apply { add(b, removeAt(a)) }
            haptics.performHapticFeedback(HapticFeedbackType.SegmentFrequentTick)
        }
    }

    LazyColumn(state = listState) {
        items(order, key = { it.id }) { playlist ->
            ReorderableItem(reorder, key = playlist.id) {
                PlaylistListRow(
                    playlist,
                    edits,
                    editing = editing,
                    handle = Modifier.draggableHandle(
                        onDragStarted = { dragging = true },
                        onDragStopped = {
                            dragging = false
                            if (order.map { it.id } != latest.map { it.id }) edits.arrange(order.map { it.id })
                        },
                    ),
                ) { open(playlist.id) }
            }
        }
    }
}

@Composable
private fun PlaylistListRow(
    playlist: Playlist,
    edits: PlaylistEdits,
    editing: Boolean,
    handle: Modifier,
    open: () -> Unit,
) {
    val app = koanApp()
    val thumb = LocalKoanTokens.current.thumb
    // Keyed on `changedAt`: adding tracks changes a playlist's face.
    val covers by produceState(emptyList<Long>(), playlist.id, playlist.changedAt) {
        value = attempt { app.mirror.engine().playlistCoverAlbumIds(playlist.id) }.getOrDefault(emptyList())
    }
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }

    Box {
        CollectionRow(
            playlist.name,
            playlist.summary(),
            onClick = if (editing) null else open,
            onLongClick = if (editing) null else ({ menu = true }),
            leading = { PlaylistMosaic(covers, thumb) },
            trailing = if (editing) ({ KoanIcon(KoanIcons.choose, "Reorder", handle.padding(KoanSpace.m)) }) else null,
        )
        KoanMenu(expanded = menu, onDismissRequest = { menu = false }) {
            KoanMenuItem("Play", KoanIcons.play) { menu = false; edits.play(playlist.id) }
            KoanMenuItem("Shuffle", KoanIcons.shuffle) { menu = false; edits.play(playlist.id, shuffled = true) }
            if (!playlist.fromFile) KoanMenuItem("Rename…") { menu = false; renaming = true }
            KoanMenuItem("Delete Playlist", KoanIcons.clear, destructive = true) { menu = false; deleting = true }
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

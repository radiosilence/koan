package cc.blit.koan.ui.playlists

import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.PageBar
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanSpace
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

/**
 * The playlists, as a page of the library; each leads to its `PlaylistPage`.
 * Edit puts a handle on each row to drag it by.
 */
@Composable
private fun PlaylistsPage() {
    val app = koanApp()
    val model = viewModel { PlaylistsModel(app.mirror) }
    val playlists by model.playlists.collectAsStateWithLifecycle()
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
        val shown = playlists
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
    // One state object for the page's life: each row's gesture outlives a
    // recomposition and keeps the state it started with, so a fresh one per
    // emission would leave the next drag moving a copy nobody draws.
    val latest by rememberUpdatedState(playlists)
    var order by remember { mutableStateOf(playlists) }
    var dragging by remember { mutableStateOf<Long?>(null) }
    var offset by remember { mutableFloatStateOf(0f) }
    // The engine's order, except under a drag: a sync landing then must not
    // move rows out from under the finger. The drop is written and comes back.
    LaunchedEffect(playlists) { if (dragging == null) order = playlists }

    fun dragBy(delta: Float) {
        offset += delta
        val visible = listState.layoutInfo.visibleItemsInfo
        val current = visible.firstOrNull { it.key == dragging } ?: return
        val middle = current.offset + offset + current.size / 2
        val target = visible.firstOrNull { it.key != current.key && middle >= it.offset && middle < it.offset + it.size } ?: return
        val from = order.indexOfFirst { it.id == dragging }
        val to = order.indexOfFirst { it.id == target.key }
        if (from < 0 || to < 0) return
        order = order.toMutableList().apply { add(to, removeAt(from)) }
        // The row is laid out at its new place next frame; keep it under the finger.
        offset += current.offset - target.offset
    }

    fun drop() {
        if (dragging != null && order.map { it.id } != latest.map { it.id }) edits.arrange(order.map { it.id })
        dragging = null
        offset = 0f
    }

    val edge = with(LocalDensity.current) { 64.dp.toPx() }
    LaunchedEffect(dragging) {
        if (dragging == null) return@LaunchedEffect
        while (true) {
            withFrameNanos {}
            val info = listState.layoutInfo
            val current = info.visibleItemsInfo.firstOrNull { it.key == dragging } ?: continue
            val top = current.offset + offset
            val step = edgeScroll(top, top + current.size, info.viewportStartOffset.toFloat(), info.viewportEndOffset.toFloat(), edge)
            if (step == 0f) continue
            offset += listState.scrollBy(step)
            dragBy(0f)
        }
    }

    LazyColumn(state = listState) {
        items(order, key = { it.id }) { playlist ->
            PlaylistListRow(
                playlist,
                edits,
                editing = editing,
                handle = Modifier.pointerInput(playlist.id) {
                    detectDragGestures(
                        onDragStart = { dragging = playlist.id; offset = 0f },
                        onDragEnd = { drop() },
                        onDragCancel = { order = latest; dragging = null; offset = 0f },
                        onDrag = { change, amount -> change.consume(); dragBy(amount.y) },
                    )
                },
                modifier = if (dragging == playlist.id) {
                    Modifier.zIndex(1f).graphicsLayer { translationY = offset }
                } else {
                    Modifier.animateItem()
                },
            ) { open(playlist.id) }
        }
    }
}

@Composable
private fun PlaylistListRow(
    playlist: Playlist,
    edits: PlaylistEdits,
    editing: Boolean,
    handle: Modifier,
    modifier: Modifier,
    open: () -> Unit,
) {
    val app = koanApp()
    val thumb = LocalKoanTokens.current.thumb
    // Keyed on `changedAt`: adding tracks changes a playlist's face.
    val covers by produceState(emptyList<Long>(), playlist.id, playlist.changedAt) {
        value = runCatching { app.mirror.engine().playlistCoverAlbumIds(playlist.id) }.getOrDefault(emptyList())
    }
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }

    Box(modifier) {
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

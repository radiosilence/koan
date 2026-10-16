package cc.blit.koan.ui.pages

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.engine.QueueSession
import cc.blit.koan.state.PlayerModel
import cc.blit.koan.state.QueueRows
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.queue.HeadingRow
import cc.blit.koan.ui.queue.QueueFormat
import cc.blit.koan.ui.queue.QueueLayout
import cc.blit.koan.ui.queue.QueueLine
import cc.blit.koan.ui.queue.QueueSelection
import cc.blit.koan.ui.queue.TrackRow
import kotlinx.coroutines.delay
import sh.calvin.reorderable.ReorderableItem
import sh.calvin.reorderable.rememberReorderableLazyListState
import uniffi.koan_ffi.QueueLock

/**
 * The queue: the main stage, the way it is in the TUI. The library is
 * somewhere you visit to feed this, not the other way round.
 *
 * Tracks sit under album headings that follow *contiguous runs* (see
 * [QueueLayout.build]). The list is laid out from the rows as last sent
 * whole, which a track change or a download does not move; each row reads its
 * own entry as the queue reads now, so those redraw the rows on screen and
 * never regroup the queue. Nothing in a row calls the engine: commands go
 * through `runtime.player`, which returns at once.
 *
 * Tap plays; a long press starts selecting (tap picks, a heading picks its
 * whole album); the handle at a row's end reorders it. Dropping sends one
 * move, and the order shown is held until the engine's new queue arrives so
 * the row does not spring back.
 *
 * Not built yet: headings cannot be dragged (move an album by picking its
 * rows), playlists are made from the playlist pages, and offline dimming
 * waits for the connection slice to say it.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun QueuePage(runtime: KoanRuntime) {
    val mirror = runtime.mirror
    val player = runtime.player
    val rows by mirror.queueRows.collectAsStateWithLifecycle()
    val queue = mirror.queue.collectAsStateWithLifecycle()
    val favourites = runtime.library.favouriteTrackIds.collectAsStateWithLifecycle()
    val playback = mirror.playback.collectAsStateWithLifecycle()
    val lock by mirror.lock.collectAsStateWithLifecycle()
    val levels = remember(runtime) { QueueSession.levels(runtime) }

    var grouped by rememberSaveable { mutableStateOf(true) }
    var selection by remember { mutableStateOf(QueueSelection()) }
    var following by remember { mutableStateOf(false) }

    val laidOut = rememberLines(rows, grouped)

    // What is shown while a row is being dragged, and until the engine's own
    // new order arrives after the drop.
    var dragged by remember { mutableStateOf<List<QueueLine>?>(null) }
    var settling by remember { mutableStateOf(false) }
    LaunchedEffect(rows) { dragged = null; settling = false }
    LaunchedEffect(settling) {
        if (settling) {
            delay(SETTLE_PATIENCE_MS)
            dragged = null
            settling = false
        }
    }
    val lines = dragged ?: laidOut
    LaunchedEffect(laidOut) { selection = selection.retain(laidOut) }

    val listState = rememberLazyListState()
    val reorder = rememberReorderableLazyListState(listState) { from, to ->
        val current = dragged ?: laidOut
        val a = current.indexOfFirst { it.id == from.key }
        val b = current.indexOfFirst { it.id == to.key }
        if (a >= 0 && b >= 0 && a != b) dragged = current.toMutableList().apply { add(b, removeAt(a)) }
    }

    // Following: what is playing, kept in view until the person scrolls.
    val touched by listState.interactionSource.collectIsDraggedAsState()
    LaunchedEffect(touched) { if (touched && following) following = false }
    LaunchedEffect(following, laidOut) {
        if (!following) return@LaunchedEffect
        snapshotFlow { playback.value.queueItemId }.collect { id ->
            QueueLayout.indexOfItem(laidOut, id).takeIf { it >= 0 }?.let { listState.centre(it) }
        }
    }

    Column(Modifier.fillMaxSize()) {
        QueueHeader(
            runtime = runtime,
            rows = rows,
            lockName = lockName(lock),
            grouped = grouped,
            onGrouped = { grouped = it },
            selection = selection,
            lines = laidOut,
            following = following,
            // Centred by the follow effect, which starts with this flag.
            onFollowing = { following = it },
            onSelection = { selection = it },
        )
        KoanRule()
        if (rows.items.isEmpty()) {
            EmptyQueue(runtime)
        } else {
            LazyColumn(state = listState, modifier = Modifier.fillMaxSize()) {
                itemsIndexed(lines, key = { _, line -> line.id }, contentType = { _, line -> line.contentType }) { _, line ->
                    ReorderableItem(reorder, key = line.id) {
                        val picked = line.id in selection.rows
                        val toggle = {
                            if (selection.active) selection = selection.toggle(line.id)
                        }
                        when (line) {
                            is QueueLine.Heading -> HeadingRow(
                                line, picked,
                                onClick = {
                                    if (selection.active) toggle() else line.group.items.firstOrNull()?.let { player.play(it.queueItemId) }
                                },
                                onLongClick = { selection = selection.toggle(line.id) },
                            )
                            is QueueLine.Track -> TrackRow(
                                runtime, levels, line, queue, favourites, picked,
                                onClick = { if (selection.active) toggle() else player.play(line.id) },
                                onLongClick = { selection = selection.toggle(line.id) },
                                dragHandle = if (selection.active) null else {
                                    {
                                        Box(
                                            Modifier
                                                .size(44.dp)
                                                .draggableHandle(
                                                    onDragStopped = {
                                                        val order = rows.items.map { it.queueItemId }
                                                        val op = QueueLayout.moveOp(
                                                            dragged ?: laidOut, line.id, selection.itemIds(laidOut).toSet(), order,
                                                        )
                                                        if (op == null) {
                                                            dragged = null
                                                        } else {
                                                            settling = true
                                                            player.move(op.itemIds, op.target, op.after)
                                                        }
                                                    },
                                                ),
                                            contentAlignment = Alignment.Center,
                                        ) { KoanIcon("chevron-up-down", tone = KoanTone.Muted, description = "Reorder") }
                                    }
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}

private const val SETTLE_PATIENCE_MS = 1500L

/** What the queue still is, when it is still something someone chose. */
private fun lockName(lock: QueueLock?): String? = when (lock) {
    is QueueLock.Playlist -> lock.playlist.name
    is QueueLock.Album -> lock.album.title
    null -> null
}

/**
 * The rows for a queue. A small queue is laid out in place; a long one on
 * the default dispatcher, so grouping five thousand rows is not the main
 * thread's work (the previous layout stays up meanwhile).
 */
@Composable
private fun rememberLines(rows: QueueRows, grouped: Boolean): List<QueueLine> {
    if (rows.items.size <= SYNC_LAYOUT_LIMIT) {
        return remember(rows, grouped) { QueueLayout.build(rows.items, grouped) }
    }
    val state = androidx.compose.runtime.produceState(emptyList<QueueLine>(), rows, grouped) {
        value = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) { QueueLayout.build(rows.items, grouped) }
    }
    return state.value
}

private const val SYNC_LAYOUT_LIMIT = 300

/** Scrolls so the row is centred: what is playing is read against what comes after it. */
private suspend fun LazyListState.centre(index: Int) {
    val viewport = layoutInfo.viewportSize.height
    animateScrollToItem(index, scrollOffset = -(viewport / 2 - ROW_ALLOWANCE_PX))
}

private const val ROW_ALLOWANCE_PX = 60

@Composable
private fun EmptyQueue(runtime: KoanRuntime) {
    val connection by runtime.mirror.connection.collectAsStateWithLifecycle()
    Column(
        Modifier.fillMaxSize().padding(KoanSpace.page),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        KoanIcon("list", tone = KoanTone.Muted, textSizeDp = 22f)
        KoanText("Queue is empty", KoanRole.Body, KoanTone.Strong, modifier = Modifier.padding(top = KoanSpace.m))
        KoanText(
            if (connection?.signInRefused == true) {
                "The server refused this device's key. Sign in again from settings."
            } else {
                "Find something to play from albums or artists."
            },
            KoanRole.Meta, KoanTone.Muted, modifier = Modifier.padding(top = KoanSpace.s), maxLines = 3,
        )
    }
}

/**
 * The title row, and under it what is wanted at the moment: the picked rows'
 * actions while selecting, the undo/redo/clear row when asked for.
 */
@Composable
private fun QueueHeader(
    runtime: KoanRuntime,
    rows: QueueRows,
    lockName: String?,
    grouped: Boolean,
    onGrouped: (Boolean) -> Unit,
    selection: QueueSelection,
    lines: List<QueueLine>,
    following: Boolean,
    onFollowing: (Boolean) -> Unit,
    onSelection: (QueueSelection) -> Unit,
) {
    val player = runtime.player
    val playing by runtime.mirror.playback.collectAsStateWithLifecycle()
    var more by remember { mutableStateOf(false) }
    val picked = remember(selection, lines) { if (selection.active) selection.itemIds(lines) else emptyList() }
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier.fillMaxWidth().padding(start = KoanSpace.page, end = KoanSpace.xs),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f).padding(vertical = KoanSpace.m)) {
                if (selection.active) {
                    KoanText("${picked.size} selected", KoanRole.Body, KoanTone.Accent)
                } else {
                    KoanText(
                        lockName?.let { QueueFormat.title(it) } ?: "queue", KoanRole.Body, KoanTone.Strong,
                        data = lockName != null,
                    )
                    KoanText(QueueFormat.summary(rows.items), KoanRole.Fine, KoanTone.Muted, data = true)
                }
            }
            if (selection.active) {
                KoanButton("all", { onSelection(selection.all(lines)) }, kind = KoanButtonKind.Text)
                KoanButton("done", { onSelection(selection.clear()) }, kind = KoanButtonKind.Text)
            } else {
                KoanIconButton(
                    "list", if (grouped) "One row per track" else "Group by album", { onGrouped(!grouped) },
                    tone = if (grouped) KoanTone.Accent else KoanTone.Muted,
                )
                KoanIconButton(
                    "play-mark", if (following) "Stop following what is playing" else "Follow what is playing",
                    { onFollowing(!following) },
                    tone = if (following) KoanTone.Accent else KoanTone.Ink, enabled = playing.queueItemId != null,
                )
                KoanIconButton("select", "Select", { onSelection(selection.copy(active = true)) }, enabled = rows.items.isNotEmpty())
                KoanIconButton("more", "More", { more = !more }, tone = if (more) KoanTone.Accent else KoanTone.Ink)
            }
        }
        if (selection.active && picked.isNotEmpty()) SelectionActions(runtime, rows, picked) { onSelection(selection.clear()) }
        if (more && !selection.active) {
            Row(Modifier.fillMaxWidth().padding(horizontal = KoanSpace.page), horizontalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                KoanButton("undo", { player.undo() }, kind = KoanButtonKind.Compact, icon = "undo")
                KoanButton("redo", { player.redo() }, kind = KoanButtonKind.Compact, icon = "redo")
                KoanButton("clear queue", { player.clearQueue(); more = false }, kind = KoanButtonKind.Destructive, icon = "trash")
            }
        }
    }
}

/** What can be done with the rows picked. Go-to-album is offered for exactly one. */
@Composable
private fun SelectionActions(runtime: KoanRuntime, rows: QueueRows, picked: List<String>, done: () -> Unit) {
    val player: PlayerModel = runtime.player
    val items = remember(rows, picked) {
        val wanted = picked.toSet()
        rows.items.filter { it.queueItemId in wanted }
    }
    val tracks = items.mapNotNull { it.trackId }
    val favourites by runtime.library.favouriteTrackIds.collectAsStateWithLifecycle()
    Row(
        Modifier.fillMaxWidth().padding(horizontal = KoanSpace.page).background(KoanTheme.colors.bg),
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        KoanButton("remove", { player.remove(picked); done() }, kind = KoanButtonKind.Destructive, icon = "remove")
        KoanButton("favourite", { tracks.filter { it !in favourites }.forEach { runtime.library.toggleFavouriteTrack(it) }; done() }, kind = KoanButtonKind.Compact, icon = "heart", enabled = tracks.isNotEmpty())
        KoanButton("download", { runtime.library.downloadToCache(tracks); done() }, kind = KoanButtonKind.Compact, icon = "download", enabled = tracks.isNotEmpty())
        items.singleOrNull()?.albumId?.let { album ->
            KoanButton("album", { runtime.navigator.openAlbum(album, items.single().trackId); done() }, kind = KoanButtonKind.Compact, icon = "album")
        }
    }
}

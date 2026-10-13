package cc.blit.koan.ui.queue

import android.content.Context
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.edit
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.R
import cc.blit.koan.art.ArtSize
import cc.blit.koan.ffi.EntryStatus
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ffi.QueueItem
import cc.blit.koan.ffi.QueueLock
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.nowplaying.art
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage
import kotlinx.coroutines.launch
import sh.calvin.reorderable.ReorderableCollectionItemScope
import sh.calvin.reorderable.ReorderableItem
import sh.calvin.reorderable.rememberReorderableLazyListState

fun EntryProviderScope<NavKey>.queueEntries() {
    entry<Tab.Queue> { QueuePage() }
}

/**
 * The queue, grouped under record headings as contiguous runs (see
 * `QueueRow.build`), or one row per track.
 *
 * A tap plays a row; a long press starts picking rows to remove or play next;
 * a swipe removes, with Undo through the engine's undo stack; the handle drags
 * a row, or a heading and its whole run, to a new place. Every edit goes to
 * the engine by queue item id, and the list redraws from what it sends back.
 */
@Composable
private fun QueuePage() {
    val app = koanApp()
    val mirror = app.mirror
    val queue by mirror.queue.collectAsStateWithLifecycle()
    val now by mirror.nowPlaying.collectAsStateWithLifecycle()
    val lock by mirror.lock.collectAsStateWithLifecycle()
    val prefs = LocalContext.current.getSharedPreferences("queue", Context.MODE_PRIVATE)
    var grouped by remember { mutableStateOf(prefs.getBoolean("grouped", true)) }
    val built = remember(queue, grouped) { QueueRow.build(queue.items, grouped) }

    // The rows as a drag has them, from its start until the engine sends the
    // queue back moved. A heading carries its run, whose rows leave the list
    // while it moves.
    var dragged by remember { mutableStateOf<List<QueueRow>?>(null) }
    LaunchedEffect(queue) { dragged = null }
    val rows = dragged ?: built
    fun lifted(row: QueueRow): List<QueueRow> {
        if (row !is QueueRow.Album) return built
        val run = row.itemIds.toSet()
        return built.filterNot { it is QueueRow.Track && it.item.queueItemId in run }
    }

    var selection by rememberSaveable { mutableStateOf(emptySet<String>()) }
    val selecting = selection.isNotEmpty()
    BackHandler(selecting) { selection = emptySet() }
    LaunchedEffect(built) { selection = selection.filterTo(mutableSetOf()) { key -> built.any { it.key == key } } }

    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    fun remove(ids: List<String>, what: String) {
        app.player.remove(ids)
        scope.launch {
            snackbar.currentSnackbarData?.dismiss()
            if (snackbar.showSnackbar("Removed $what", "Undo", duration = SnackbarDuration.Short) == SnackbarResult.ActionPerformed) {
                mirror.launch { undo(null) }
            }
        }
    }

    // Following: the playing row kept in view as it moves on, until the
    // listener scrolls the list themselves.
    val list = rememberLazyListState()
    var following by remember { mutableStateOf(false) }
    val current = now?.queueItemId
    LaunchedEffect(list) {
        list.interactionSource.interactions.collect { if (it is DragInteraction.Start) following = false }
    }
    LaunchedEffect(following, current, built) {
        if (!following || current == null) return@LaunchedEffect
        val index = built.indexOfFirst { it.key == current }
        if (index >= 0) list.animateScrollToItem(index, -list.layoutInfo.viewportSize.height / 3)
    }
    LaunchedEffect(current == null) { if (current == null) following = false }

    val haptics = LocalHapticFeedback.current
    val reorder = rememberReorderableLazyListState(list) { from, to ->
        val drawn = (dragged ?: built).toMutableList()
        val a = drawn.indexOfFirst { it.key == from.key }
        val b = drawn.indexOfFirst { it.key == to.key }
        if (a >= 0 && b >= 0) {
            drawn.add(b, drawn.removeAt(a))
            dragged = drawn
            haptics.performHapticFeedback(HapticFeedbackType.SegmentFrequentTick)
        }
    }
    fun drop(row: QueueRow) {
        val drawn = dragged ?: return
        val moving = row.itemIds.toSet()
        val target = moveTarget(drawn, row.key, moving, queue.items)
        // Let go where it started: nothing to send, and nothing will come back.
        if (target == null || target == moveTarget(lifted(row), row.key, moving, queue.items)) {
            dragged = null
            return
        }
        mirror.launch { moveInQueue(row.itemIds, target.first, target.second) }
    }

    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            if (selecting) {
                SelectionBar(
                    count = QueueRow.itemIds(selection, built).size,
                    clear = { selection = emptySet() },
                    // Already queued, so play next is a move to just after what is playing.
                    playNext = {
                        val ids = QueueRow.itemIds(selection, built).filter { it != current }
                        if (current != null && ids.isNotEmpty()) mirror.launch { moveInQueue(ids, current, true) }
                        selection = emptySet()
                    },
                    remove = {
                        val ids = QueueRow.itemIds(selection, built)
                        selection = emptySet()
                        remove(ids, count(ids.size, "track"))
                    },
                )
            } else {
                Header(
                    queue.items,
                    lock,
                    grouped,
                    setGrouped = {
                        grouped = it
                        prefs.edit { putBoolean("grouped", it) }
                    },
                    following = following,
                    follow = { following = !following },
                    canFollow = current != null,
                    clear = { mirror.launch { clearQueue() } },
                )
            }
            if (queue.items.isEmpty()) {
                StubPage("Queue is empty")
                return@Column
            }
            val playing = now?.state == PlayState.PLAYING
            LazyColumn(Modifier.fillMaxSize(), state = list) {
                items(rows, key = { it.key }) { row ->
                    ReorderableItem(reorder, key = row.key) { isDragging ->
                        val picked = row.key in selection
                        Swipeable(enabled = !selecting && !isDragging, onRemove = { remove(row.itemIds, row.label) }) {
                            Surface(
                                shadowElevation = if (isDragging) 6.dp else 0.dp,
                                color = if (picked) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surface,
                            ) {
                                RowContent(
                                    row,
                                    grouped,
                                    playing,
                                    selecting,
                                    picked,
                                    tap = {
                                        if (selecting) {
                                            selection = if (picked) selection - row.key else selection + row.key
                                        } else {
                                            row.itemIds.firstOrNull()?.let(app.player::play)
                                        }
                                    },
                                    press = {
                                        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                        selection = selection + row.key
                                    },
                                    onDrag = { dragged = lifted(row) },
                                    onDrop = { drop(row) },
                                )
                            }
                        }
                    }
                }
            }
        }
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter))
    }
}

@Composable
private fun Swipeable(enabled: Boolean, onRemove: () -> Unit, content: @Composable () -> Unit) {
    val state = rememberSwipeToDismissBoxState()
    SwipeToDismissBox(
        state,
        backgroundContent = {
            Box(
                Modifier.fillMaxSize().background(MaterialTheme.colorScheme.errorContainer).padding(horizontal = 24.dp),
                contentAlignment = if (state.dismissDirection == SwipeToDismissBoxValue.StartToEnd) Alignment.CenterStart else Alignment.CenterEnd,
            ) {
                Icon(painterResource(R.drawable.ic_delete), "Remove", tint = MaterialTheme.colorScheme.onErrorContainer)
            }
        },
        gesturesEnabled = enabled,
        onDismiss = { onRemove() },
    ) { content() }
}

@Composable
private fun ReorderableCollectionItemScope.RowContent(
    row: QueueRow,
    grouped: Boolean,
    playing: Boolean,
    selecting: Boolean,
    picked: Boolean,
    tap: () -> Unit,
    press: () -> Unit,
    onDrag: () -> Unit,
    onDrop: () -> Unit,
) {
    val tokens = LocalKoanTokens.current
    Row(
        Modifier.fillMaxWidth()
            .combinedClickable(onClick = tap, onLongClick = press)
            .semantics { selected = picked }
            .padding(start = tokens.gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (selecting) {
            Icon(
                painterResource(if (picked) R.drawable.ic_select else R.drawable.ic_unselected),
                null,
                Modifier.padding(end = tokens.rowGap),
                tint = if (picked) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
            )
        }
        Box(Modifier.weight(1f)) {
            when (row) {
                is QueueRow.Album -> AlbumHeading(row.group)
                is QueueRow.Single -> TrackRow(row.item, playing, sleeve = true, showArtist = true)
                is QueueRow.Track -> TrackRow(
                    row.item,
                    playing,
                    // Ungrouped there is no heading above to say what record this is.
                    sleeve = !grouped,
                    showArtist = !grouped || row.item.artist != row.item.albumArtist,
                )
            }
        }
        IconButton(onClick = {}, Modifier.draggableHandle(onDragStarted = { onDrag() }, onDragStopped = onDrop)) {
            Icon(painterResource(R.drawable.ic_drag), "Reorder", tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun AlbumHeading(group: QueueGroup) {
    val tokens = LocalKoanTokens.current
    Row(Modifier.padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        val sleeve = Modifier.size(52.dp).clip(RoundedCornerShape(5.dp))
        group.items.first().art(ArtSize.Thumb)?.let { AsyncImage(it, null, sleeve) }
            ?: Box(sleeve.background(MaterialTheme.colorScheme.surfaceVariant))
        Spacer(Modifier.width(tokens.rowGap * 1.5f))
        Column {
            Text(group.title, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
            if (group.album.isNotEmpty()) {
                Text(
                    group.albumArtist.ifEmpty { "Unknown Artist" },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(group.detail, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.outline, maxLines = 1)
        }
    }
}

@Composable
private fun TrackRow(item: QueueItem, playing: Boolean, sleeve: Boolean, showArtist: Boolean) {
    val tokens = LocalKoanTokens.current
    val played = item.status == EntryStatus.PLAYED
    val current = item.status == EntryStatus.PLAYING
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    Row(
        Modifier.heightIn(min = if (sleeve) 60.dp else 48.dp).padding(vertical = if (sleeve) 6.dp else 0.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(20.dp), contentAlignment = Alignment.CenterEnd) { StatusMark(item, playing) }
        Spacer(Modifier.width(if (sleeve) 10.dp else 6.dp))
        if (sleeve) {
            val shape = Modifier.size(tokens.thumb).clip(RoundedCornerShape(4.dp))
            item.art(ArtSize.Thumb)?.let { AsyncImage(it, null, shape, alpha = if (played) 0.5f else 1f) }
                ?: Box(shape.background(MaterialTheme.colorScheme.surfaceVariant))
        } else {
            Text(
                item.trackNumber?.toString() ?: "",
                Modifier.width(22.dp),
                style = MaterialTheme.typography.labelMedium,
                color = if (played) MaterialTheme.colorScheme.outlineVariant else MaterialTheme.colorScheme.outline,
            )
        }
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text(
                item.title,
                style = MaterialTheme.typography.bodyLarge,
                fontWeight = if (current) FontWeight.SemiBold else null,
                color = when {
                    current -> MaterialTheme.colorScheme.primary
                    played -> muted
                    else -> MaterialTheme.colorScheme.onSurface
                },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (showArtist && item.artist.isNotEmpty()) {
                Text(
                    if (sleeve && item.album.isNotEmpty()) "${item.artist} — ${item.album}" else item.artist,
                    style = MaterialTheme.typography.bodySmall,
                    color = muted,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Availability(item)
        item.durationMs?.let {
            Text(duration(it.toLong()), Modifier.padding(start = 8.dp), style = MaterialTheme.typography.labelMedium, color = muted)
        }
    }
}

/** What the queue is doing about this track: playing, done, waiting, failed. Arrival is `Availability`'s. */
@Composable
private fun StatusMark(item: QueueItem, playing: Boolean) {
    val size = Modifier.size(16.dp)
    val colours = MaterialTheme.colorScheme
    when (item.status) {
        EntryStatus.PLAYING -> Icon(
            painterResource(R.drawable.ic_playing),
            if (playing) "Playing" else "Paused",
            size,
            tint = if (playing) colours.primary else colours.onSurfaceVariant,
        )
        EntryStatus.PLAYED -> Icon(painterResource(R.drawable.ic_check), "Played", size, tint = colours.outline)
        EntryStatus.PRIORITY_PENDING -> Icon(painterResource(R.drawable.ic_download), "Queued for download", size, tint = colours.primary)
        EntryStatus.FAILED -> Icon(painterResource(R.drawable.ic_warning), item.failureReason ?: "Couldn't be fetched", size, tint = colours.error)
        EntryStatus.QUEUED -> Icon(painterResource(R.drawable.ic_pending), null, size, tint = colours.outlineVariant)
        EntryStatus.DOWNLOADING -> Unit
    }
}

/**
 * Where the track's bytes are: arriving, with how far along, or only on the
 * server. Its own slot, since a track can be playing and still arriving.
 * Only a downloading row reads the transfer figures, which move many times a
 * second.
 */
@Composable
private fun Availability(item: QueueItem) {
    val trackId = item.trackId
    when {
        item.status == EntryStatus.DOWNLOADING && trackId != null -> {
            val figures by koanApp().mirror.figures.collectAsStateWithLifecycle()
            val progress = figures.firstOrNull { it.trackId == trackId }?.progress
            Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
                if (progress != null) {
                    CircularProgressIndicator(progress = { progress.toFloat() }, Modifier.size(16.dp), strokeWidth = 2.dp)
                } else {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                }
            }
        }
        item.onServer && !item.onDisk -> Icon(
            painterResource(R.drawable.ic_cloud),
            "On the server",
            Modifier.size(16.dp),
            tint = MaterialTheme.colorScheme.outline,
        )
    }
}

@Composable
private fun Header(
    items: List<QueueItem>,
    lock: QueueLock?,
    grouped: Boolean,
    setGrouped: (Boolean) -> Unit,
    following: Boolean,
    follow: () -> Unit,
    canFollow: Boolean,
    clear: () -> Unit,
) {
    val tokens = LocalKoanTokens.current
    Row(Modifier.fillMaxWidth().padding(start = tokens.gutter, top = 8.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            // What the queue still is, when it came from a playlist or a record and has not been edited since.
            val name = when (lock) {
                is QueueLock.Playlist -> lock.playlist.name
                is QueueLock.Album -> lock.album.title
                null -> "Queue"
            }
            Text(name, style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (items.isNotEmpty()) {
                val total = items.sumOf { it.durationMs?.toLong() ?: 0 }
                Text(
                    count(items.size, "track") + if (total > 0) " · ${duration(total)}" else "",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        IconButton(onClick = follow, enabled = canFollow) {
            Icon(
                painterResource(R.drawable.ic_jump),
                if (following) "Stop following what's playing" else "Jump to what's playing",
                tint = if (following) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        IconButton(onClick = { setGrouped(!grouped) }) {
            Icon(
                painterResource(if (grouped) R.drawable.ic_album else R.drawable.ic_list),
                if (grouped) "Grouped by record; show one row per track" else "One row per track; group by record",
            )
        }
        var menu by remember { mutableStateOf(false) }
        Box {
            IconButton(onClick = { menu = true }) { Icon(painterResource(R.drawable.ic_more), "More") }
            DropdownMenu(menu, onDismissRequest = { menu = false }) {
                DropdownMenuItem(
                    text = { Text("Clear queue") },
                    leadingIcon = { Icon(painterResource(R.drawable.ic_delete), null) },
                    enabled = items.isNotEmpty(),
                    onClick = {
                        menu = false
                        clear()
                    },
                )
            }
        }
    }
}

@Composable
private fun SelectionBar(count: Int, clear: () -> Unit, playNext: () -> Unit, remove: () -> Unit) {
    Surface(tonalElevation = 3.dp) {
        Row(
            Modifier.fillMaxWidth().padding(4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            IconButton(onClick = clear) { Icon(painterResource(R.drawable.ic_close), "Clear selection") }
            Text("$count selected", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            TextButton(onClick = playNext) {
                Icon(painterResource(R.drawable.ic_play_next), null, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text("Play next")
            }
            TextButton(onClick = remove) {
                Icon(painterResource(R.drawable.ic_delete), null, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text("Remove")
            }
        }
    }
}

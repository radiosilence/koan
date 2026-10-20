package cc.blit.koan.ui.queue

import android.content.Context
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.foundation.interaction.MutableInteractionSource
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
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.SwipeToDismissBoxDefaults
import androidx.compose.material3.SwipeToDismissBoxState
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.edit
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.R
import cc.blit.koan.art.ArtSize
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.EntryStatus
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ffi.QueueItem
import cc.blit.koan.ffi.QueueLock
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.Cover
import cc.blit.koan.ui.library.NameLine
import cc.blit.koan.ui.library.Playable
import cc.blit.koan.ui.library.PlayableMenu
import cc.blit.koan.ui.library.PlayingBars
import cc.blit.koan.ui.library.albumName
import cc.blit.koan.ui.library.artistCredit
import cc.blit.koan.ui.library.artistName
import cc.blit.koan.ui.library.claimsTaps
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.nowplaying.art
import cc.blit.koan.ui.playlists.AddToPlaylistSheet
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanDivider
import cc.blit.koan.ui.theme.KoanEdge
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIconToggle
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanSurface
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.theme.koanRule
import cc.blit.koan.ui.theme.koanSurface
import kotlinx.coroutines.delay
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
 * A tap plays a row; a long press opens its menu (the library's, for the
 * track or the record); Select in the header starts picking rows to remove or
 * play next;
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
    var saving by remember { mutableStateOf(false) }
    if (saving) AddToPlaylistSheet({ queue.items.mapNotNull { it.trackId } }) { saving = false }
    var selecting by rememberSaveable { mutableStateOf(false) }
    fun stopSelecting() {
        selecting = false
        selection = emptySet()
    }
    BackHandler(selecting) { stopSelecting() }
    LaunchedEffect(built) { selection = selection.filterTo(mutableSetOf()) { key -> built.any { it.key == key } } }

    val scope = rememberCoroutineScope()
    // What the last removal took, offered back for a few seconds.
    var removed by remember { mutableStateOf<Removal?>(null) }
    fun remove(ids: List<String>, what: String) {
        app.player.remove(ids)
        removed = Removal(what)
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
                    clear = ::stopSelecting,
                    // Already queued, so play next is a move to just after what is playing.
                    playNext = {
                        val ids = QueueRow.itemIds(selection, built).filter { it != current }
                        if (current != null && ids.isNotEmpty()) mirror.launch { moveInQueue(ids, current, true) }
                        stopSelecting()
                    },
                    remove = {
                        val ids = QueueRow.itemIds(selection, built)
                        stopSelecting()
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
                    select = { selecting = true },
                    save = { saving = true },
                    clear = { mirror.launch { clearQueue() } },
                )
            }
            if (queue.items.isEmpty()) {
                KoanUnavailable("Queue is empty", KoanIcons.queueSection, "Find something to play from the library.")
                return@Column
            }
            val playing = now?.state == PlayState.PLAYING
            LazyColumn(Modifier.fillMaxSize(), state = list) {
                items(rows, key = { it.key }) { row ->
                    ReorderableItem(reorder, key = row.key) { isDragging ->
                        val picked = row.key in selection
                        var menu by remember { mutableStateOf<Playable?>(null) }
                        Swipeable(enabled = !selecting && !isDragging, onRemove = { remove(row.itemIds, row.label) }) { swiping ->
                            // A lifted row stands on `surface` rather than a shadow; one being
                            // swiped on `bg`, over what the swipe uncovers. Otherwise on the wash.
                            val ground = when {
                                isDragging || picked -> Modifier.koanSurface(KoanSurface.Surface)
                                swiping -> Modifier.koanSurface()
                                else -> Modifier
                            }
                            Box(ground.systemGround(isDragging || picked)) {
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
                                        scope.launch { menu = mirror.playable(row) }
                                    },
                                    onDrag = { dragged = lifted(row) },
                                    onDrop = { drop(row) },
                                )
                                menu?.let { PlayableMenu(it, true) { menu = null } }
                            }
                        }
                    }
                }
            }
        }
        removed?.let { removal ->
            UndoBar(removal, Modifier.align(Alignment.BottomCenter), onTimeout = { if (removed === removal) removed = null }) {
                removed = null
                mirror.launch { undo(null) }
            }
        }
    }
}

/** The platform look's ground for a lifted or picked row; the theme's is `koanSurface`. */
@Composable
private fun Modifier.systemGround(raised: Boolean) =
    if (!KoanTheme.isOn && raised) background(MaterialTheme.colorScheme.secondaryContainer) else this

/**
 * A row that a swipe removes. Its state is remembered, not saved: a list
 * keeps saved state by key, and a row put back by Undo has the same key, so a
 * saved state would come back dismissed and remove the row again at once.
 */
@Composable
internal fun Swipeable(enabled: Boolean, onRemove: () -> Unit, content: @Composable (swiping: Boolean) -> Unit) {
    val threshold = SwipeToDismissBoxDefaults.positionalThreshold
    val state = remember { SwipeToDismissBoxState(SwipeToDismissBoxValue.Settled, threshold) }
    SwipeToDismissBox(
        state,
        backgroundContent = {
            if (state.dismissDirection == SwipeToDismissBoxValue.Settled) return@SwipeToDismissBox
            Box(
                Modifier.fillMaxSize().background(KoanTheme.color(KoanTone.Bad)).padding(horizontal = KoanSpace.xl),
                contentAlignment = if (state.dismissDirection == SwipeToDismissBoxValue.StartToEnd) Alignment.CenterStart else Alignment.CenterEnd,
            ) {
                KoanIcon(KoanIcons.remove, "Remove", color = KoanTheme.colors.bg)
            }
        },
        gesturesEnabled = enabled,
        onDismiss = { onRemove() },
    ) { content(state.dismissDirection != SwipeToDismissBoxValue.Settled) }
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
    Row(
        Modifier.fillMaxWidth()
            .combinedClickable(remember { MutableInteractionSource() }, indication = null, onClick = tap, onLongClick = press)
            .semantics { selected = picked }
            .padding(start = KoanSpace.l),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (selecting) {
            KoanIcon(
                if (picked) KoanIcons.picked else KoanIcons.unpicked,
                null,
                Modifier.padding(end = KoanSpace.s),
                color = KoanTheme.color(if (picked) KoanTone.Accent else KoanTone.Muted),
            )
        }
        Box(Modifier.weight(1f)) {
            when (row) {
                is QueueRow.Album -> AlbumHeading(row.group, linked = !selecting)
                is QueueRow.Single -> TrackRow(row.item, playing, sleeve = true, showArtist = true, linked = !selecting)
                is QueueRow.Track -> TrackRow(
                    row.item,
                    playing,
                    linked = !selecting,
                    // Ungrouped there is no heading above to say what record this is.
                    sleeve = !grouped,
                    showArtist = !grouped || row.item.artist != row.item.albumArtist,
                )
            }
        }
        KoanButton({}, Modifier.draggableHandle(onDragStarted = { onDrag() }, onDragStopped = onDrop), kind = KoanButtonKind.Icon) {
            Icon(painterResource(R.drawable.ic_drag), "Reorder", tint = KoanTheme.color(KoanTone.Muted))
        }
    }
}

@Composable
private fun sleeveShape(radius: Int) = if (KoanTheme.isOn) RectangleShape else RoundedCornerShape(radius.dp)

@Composable
private fun AlbumHeading(group: QueueGroup, linked: Boolean) {
    Row(Modifier.padding(vertical = KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
        val sleeve = Modifier.size(52.dp).clip(sleeveShape(5))
        Cover(group.items.first().art(ArtSize.Thumb), sleeve)
        Spacer(Modifier.width(KoanSpace.m))
        Column {
            val first = group.items.first()
            if (group.album.isNotEmpty()) {
                NameLine(listOf(albumName(group.title, first.albumId)), role = KoanType.Body, tone = KoanTone.Strong, linked = linked, maxLines = 2)
                val artist = when {
                    group.albumArtist.isEmpty() -> artistName("Unknown Artist", first.albumArtistId)
                    else -> artistCredit(first.albumArtistCredit, group.albumArtist, first.albumArtistId)
                }
                NameLine(listOf(artist), linked = linked)
            } else {
                KoanText(group.title, role = KoanType.Body, tone = KoanTone.Strong, maxLines = 2)
            }
            KoanText(group.detail, role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 1)
        }
    }
}

@Composable
private fun TrackRow(item: QueueItem, playing: Boolean, sleeve: Boolean, showArtist: Boolean, linked: Boolean) {
    val tokens = LocalKoanTokens.current
    val played = item.status == EntryStatus.PLAYED
    val current = item.status == EntryStatus.PLAYING
    Row(
        Modifier.heightIn(min = if (sleeve) 60.dp else 48.dp).padding(vertical = if (sleeve) 6.dp else 0.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(20.dp), contentAlignment = Alignment.CenterEnd) { StatusMark(item, playing) }
        Spacer(Modifier.width(if (sleeve) 10.dp else 6.dp))
        if (sleeve) {
            val shape = Modifier.size(tokens.thumb).clip(sleeveShape(4))
            Cover(item.art(ArtSize.Thumb), shape.alpha(if (played) 0.5f else 1f))
        } else {
            KoanText(item.trackNumber?.toString() ?: "", Modifier.width(22.dp), role = KoanType.Fine, tone = KoanTone.Muted)
        }
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            KoanText(
                item.title,
                Modifier.fillMaxWidth().claimsTaps(),
                tone = when {
                    current -> KoanTone.Accent
                    played -> KoanTone.Muted
                    else -> KoanTone.Ink
                },
                maxLines = 1,
            )
            if (showArtist && item.artist.isNotEmpty()) {
                NameLine(
                    listOfNotNull(
                        artistCredit(item.artistCredit, item.artist, item.artistId),
                        albumName(item.album, item.albumId, item.trackId).takeIf { sleeve },
                    ),
                    role = KoanType.Fine,
                    linked = linked,
                )
            }
        }
        Availability(item)
        item.durationMs?.let {
            KoanText(duration(it.toLong()), Modifier.padding(start = KoanSpace.s), role = KoanType.Fine, tone = KoanTone.Muted)
        }
    }
}

/** What the queue is doing about this track: playing, done, waiting, failed. Arrival is `Availability`'s. */
@Composable
private fun StatusMark(item: QueueItem, playing: Boolean) {
    val size = 12.sp
    // Still to come but held up, the server out of reach: said, not left a
    // silent wait.
    val heldUp = item.failureReason.takeIf { item.status != EntryStatus.FAILED && item.status != EntryStatus.PLAYING }
    if (heldUp != null) {
        return KoanIcon(KoanIcons.warningFilled, heldUp, size = size, color = KoanTheme.color(KoanTone.Bad))
    }
    when (item.status) {
        EntryStatus.PLAYING -> PlayingBars(
            playing,
            Modifier.semantics { contentDescription = if (playing) "Playing" else "Paused" },
            size = 12.dp,
            color = KoanTheme.color(if (playing) KoanTone.Accent else KoanTone.Muted),
        )
        EntryStatus.PLAYED -> KoanIcon(KoanIcons.check, "Played", size = size, color = KoanTheme.color(KoanTone.Muted))
        EntryStatus.PRIORITY_PENDING ->
            KoanIcon(KoanIcons.downloads, "Queued for download", size = size, color = KoanTheme.color(KoanTone.Accent))
        EntryStatus.FAILED ->
            KoanIcon(KoanIcons.warningFilled, item.failureReason ?: "Couldn't be fetched", size = size, color = KoanTheme.color(KoanTone.Bad))
        EntryStatus.QUEUED -> KoanIcon(KoanIcons.pending, null, size = size, color = KoanTheme.color(KoanTone.Rule))
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
            val colour = KoanTheme.look.accent
            Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
                if (progress != null) {
                    CircularProgressIndicator(progress = { progress.toFloat() }, Modifier.size(14.dp), color = colour, strokeWidth = 2.dp)
                } else {
                    CircularProgressIndicator(Modifier.size(14.dp), color = colour, strokeWidth = 2.dp)
                }
            }
        }
        item.onServer && !item.onDisk -> KoanIcon(KoanIcons.cloud, "On the server", size = 12.sp, color = KoanTheme.color(KoanTone.Muted))
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
    select: () -> Unit,
    save: () -> Unit,
    clear: () -> Unit,
) {
    Row(Modifier.fillMaxWidth().padding(start = KoanSpace.l, top = KoanSpace.s, bottom = KoanSpace.xs), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            // What the queue still is, when it came from a playlist or a record and has not been edited since.
            when (lock) {
                is QueueLock.Playlist -> KoanText(lock.playlist.name, role = KoanType.Body, tone = KoanTone.Strong, maxLines = 1)
                is QueueLock.Album -> NameLine(listOf(albumName(lock.album.title, lock.album.id)), role = KoanType.Body, tone = KoanTone.Strong)
                null -> KoanText("Queue", role = KoanType.Body, tone = KoanTone.Strong, appText = true)
            }
            if (items.isNotEmpty()) {
                val total = items.sumOf { it.durationMs?.toLong() ?: 0 }
                KoanText(count(items.size, "track") + if (total > 0) " · ${duration(total)}" else "", role = KoanType.Fine, tone = KoanTone.Muted)
            }
        }
        KoanButton(follow, kind = KoanButtonKind.Icon, enabled = canFollow) {
            KoanIcon(
                KoanIcons.jumpToPlaying,
                if (following) "Stop following what's playing" else "Jump to what's playing",
                color = KoanTheme.color(if (following) KoanTone.Accent else KoanTone.Ink),
            )
        }
        KoanIconToggle(KoanIcons.album, "Group by album", grouped, setGrouped)
        KoanButton("Select", select, kind = KoanButtonKind.Icon, icon = KoanIcons.select, enabled = items.isNotEmpty())
        var menu by remember { mutableStateOf(false) }
        Box {
            KoanButton("More", { menu = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.more)
            KoanMenu(menu, onDismissRequest = { menu = false }) {
                KoanMenuItem("Save as Playlist…", KoanIcons.playlist, enabled = items.isNotEmpty()) {
                    menu = false
                    save()
                }
                KoanDivider()
                KoanMenuItem("Clear Queue", KoanIcons.clear, destructive = true, enabled = items.isNotEmpty()) {
                    menu = false
                    clear()
                }
            }
        }
    }
}

@Composable
private fun SelectionBar(count: Int, clear: () -> Unit, playNext: () -> Unit, remove: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().koanSurface(KoanSurface.Surface).koanRule(KoanEdge.Bottom).padding(horizontal = KoanSpace.xs),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs),
    ) {
        KoanButton("Clear selection", clear, kind = KoanButtonKind.Icon, icon = KoanIcons.deselect)
        KoanText("$count selected", Modifier.weight(1f), role = KoanType.Meta, tone = KoanTone.Strong)
        KoanButton("Play next", playNext, kind = KoanButtonKind.Text, icon = KoanIcons.playNext)
        KoanButton("Remove", remove, kind = KoanButtonKind.Text, icon = KoanIcons.remove)
    }
}

/** What a row's menu acts on: the track it plays, or the record a heading names, as the library has them. */
private suspend fun EngineMirror.playable(row: QueueRow): Playable? = runCatching {
    when (row) {
        is QueueRow.Album -> row.group.items.first().albumId?.let { engine().album(it) }?.let(Playable::OfAlbum)
        is QueueRow.Track -> row.item.trackId?.let { engine().track(it) }?.let(Playable::OfTrack)
        is QueueRow.Single -> row.item.trackId?.let { engine().track(it) }?.let(Playable::OfTrack)
    }
}.getOrNull()

/** One removal, by identity, so a second one restarts the bar's time. */
class Removal(val what: String)

/**
 * "Removed X · Undo" along the foot of the queue for a few seconds after a
 * removal, taking it back through the engine's undo stack. Square on
 * `surface` with a top rule in the theme, no shadow.
 */
@Composable
fun UndoBar(removal: Removal, modifier: Modifier = Modifier, onTimeout: () -> Unit, onUndo: () -> Unit) {
    LaunchedEffect(removal) {
        delay(5_000)
        onTimeout()
    }
    Row(
        modifier.fillMaxWidth().koanSurface(KoanSurface.Surface).systemGround(true).koanRule(KoanEdge.Top)
            .padding(start = KoanSpace.l, end = KoanSpace.xs),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        KoanText("Removed ${removal.what}", Modifier.weight(1f), role = KoanType.Meta, maxLines = 1)
        KoanButton("Undo", onUndo)
    }
}

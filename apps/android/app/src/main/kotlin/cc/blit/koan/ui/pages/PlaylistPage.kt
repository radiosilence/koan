package cc.blit.koan.ui.pages

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.engine.EngineDispatcher
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import cc.blit.koan.ui.playlists.ConfirmDeleteDialog
import cc.blit.koan.ui.playlists.NameDialog
import cc.blit.koan.ui.playlists.NewPlaylistHost
import cc.blit.koan.ui.playlists.PlaylistLayout
import cc.blit.koan.ui.playlists.PlaylistLine
import cc.blit.koan.ui.playlists.PlaylistMosaic
import cc.blit.koan.ui.playlists.PlaylistReport
import cc.blit.koan.ui.playlists.entryKey
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import sh.calvin.reorderable.ReorderableItem
import sh.calvin.reorderable.rememberReorderableLazyListState
import uniffi.koan_ffi.PlaylistEntry
import uniffi.koan_ffi.Track

/**
 * One playlist, laid out like the queue. The port of `PlaylistView.swift`.
 *
 * Grouped puts a heading over each contiguous run of one record; flat is a row
 * per entry. Ungrouped by default, unlike the queue: a playlist is a sequence
 * someone chose, and the choice is remembered per playlist (on this device).
 * A row's identity is its *entry*, since a playlist may hold a track twice.
 * What is playing is mirrored onto the rows (the queue item's playlist entry),
 * not owned here.
 *
 * Tap plays from that entry with the rest of the playlist behind it; a long
 * press starts selecting (a heading picks its whole run). The handle at a
 * row's end reorders it; the drop sends one full order and the order shown is
 * held until the model's reread arrives so the row does not spring back.
 *
 * A read-only playlist (rules or a file decide what is in it) shows no handle,
 * no remove and no shuffle-order: the model would refuse them, and the page
 * does not offer what will be refused. Rename is separate, as in the model:
 * only a playlist read from a file keeps its name.
 *
 * The header is the page's own for now. When the album page's shared record
 * header lands the two should be unified (cover as wide as the first column,
 * text from the second).
 *
 * Not built yet: dropping tracks in from elsewhere (`PlaylistsModel.insert`
 * has no source on a phone yet), playlist export (needs the Storage Access
 * Framework), an "add to playlist" entry on the rows (needs the menu shared
 * with the other pages).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun PlaylistPage(runtime: KoanRuntime, playlistId: Long) {
    val model = runtime.playlists
    val player = runtime.player
    val all by model.playlists.collectAsStateWithLifecycle()
    val openId by model.openId.collectAsStateWithLifecycle()
    val loaded by model.entries.collectAsStateWithLifecycle()
    val loading by model.isLoading.collectAsStateWithLifecycle()
    val covers by model.covers.collectAsStateWithLifecycle()
    val libraryVersion by runtime.mirror.libraryVersion.collectAsStateWithLifecycle()
    val queue = runtime.mirror.queue.collectAsStateWithLifecycle()
    val playback = runtime.mirror.playback.collectAsStateWithLifecycle()
    val favourites by runtime.library.favouriteTrackIds.collectAsStateWithLifecycle()

    // The rows arrived before the page did; this is for a library change after.
    // On the engine's dispatcher, where the model is confined.
    LaunchedEffect(playlistId, libraryVersion) {
        withContext(EngineDispatcher.dispatcher) { model.prepare(playlistId) }
    }

    val playlist = all.firstOrNull { it.id == playlistId }
    // Guarded on the id because history can move faster than a read.
    val entries = if (openId == playlistId) loaded else emptyList()
    val fillable = PlaylistLayout.fillable(playlist)
    val grouped = playlist?.grouped ?: false

    // Which entry is lit: read as a derived value, so a queue patch that does
    // not move it does not redraw the page.
    val playingEntry by remember {
        derivedStateOf { playback.value.queueItemId?.let { queue.value.item(it) }?.playlistEntryId }
    }

    val laidOut = rememberLines(entries, grouped)

    var selecting by remember(playlistId) { mutableStateOf(false) }
    var picked by remember(playlistId) { mutableStateOf<Set<String>>(emptySet()) }
    LaunchedEffect(laidOut) {
        val live = laidOut.mapTo(HashSet()) { it.id }
        if (!live.containsAll(picked)) picked = picked.filterTo(HashSet()) { it in live }
    }

    var more by remember(playlistId) { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }

    var dragged by remember { mutableStateOf<List<PlaylistLine>?>(null) }
    LaunchedEffect(entries) { dragged = null }
    LaunchedEffect(dragged) {
        if (dragged != null) {
            delay(SETTLE_PATIENCE_MS)
            dragged = null
        }
    }
    val lines = dragged ?: laidOut

    val listState = rememberLazyListState()
    val reorder = rememberReorderableLazyListState(listState) { from, to ->
        val current = dragged ?: laidOut
        val a = current.indexOfFirst { it.id == from.key }
        val b = current.indexOfFirst { it.id == to.key }
        if (a >= 0 && b >= 0 && a != b) dragged = current.toMutableList().apply { add(b, removeAt(a)) }
    }

    fun playFrom(position: Int) {
        val ids = entries.map { it.track.id }
        if (ids.isNotEmpty()) player.playNow(ids, startingAt = position)
    }

    val pickedPositions = remember(picked, laidOut) { PlaylistLayout.positions(laidOut, picked) }
    val pickedTracks = remember(pickedPositions, entries) { PlaylistLayout.trackIds(entries, pickedPositions) }

    Column(Modifier.fillMaxSize()) {
        LazyColumn(state = listState, modifier = Modifier.fillMaxSize()) {
            item(key = "header", contentType = "header") {
                Header(
                    name = playlist?.name ?: "Playlist",
                    summary = PlaylistLayout.summary(entries, synced = playlist?.remoteId != null),
                    note = when {
                        playlist == null -> null
                        playlist.smart -> "Its rules decide what is in it."
                        playlist.fromFile -> "Read from a file in the library."
                        playlist.readonly -> "Kept by the server; it takes no edits here."
                        else -> null
                    },
                    cover = covers[playlistId].orEmpty(),
                    hasEntries = entries.isNotEmpty(),
                    fillable = fillable,
                    renamable = PlaylistLayout.renamable(playlist),
                    grouped = grouped,
                    selecting = selecting,
                    more = more,
                    picked = pickedPositions.size,
                    onPlay = { playFrom(0) },
                    onShuffle = { player.playNow(playlist?.name ?: "playlist", shuffled = true) { entries.map { it.track.id } } },
                    onPlayNext = { player.playNext(entries.map { it.track.id }) },
                    onQueue = { player.enqueue(entries.map { it.track.id }) },
                    onGrouped = { model.setGrouped(it, playlistId) },
                    onSelect = { selecting = true },
                    onMore = { more = !more },
                    onRename = { renaming = true; more = false },
                    onShuffleOrder = { model.shuffle(playlistId); more = false },
                    onDelete = { deleting = true; more = false },
                    onSelectAll = { picked = laidOut.mapTo(HashSet()) { it.id } },
                    onDone = { selecting = false; picked = emptySet() },
                    onPickedNext = { player.playNext(pickedTracks) },
                    onPickedQueue = { player.enqueue(pickedTracks) },
                    onPickedRemove = {
                        model.remove(PlaylistLayout.entryIds(entries, pickedPositions), playlistId)
                        selecting = false
                        picked = emptySet()
                    },
                )
            }
            if (entries.isEmpty()) {
                item(key = "empty", contentType = "empty") {
                    EmptyPlaylist(
                        title = if (loading) "Loading" else "Nothing in here yet",
                        detail = when {
                            playlist?.smart == true -> "Nothing in the library matches its rules yet."
                            playlist?.readonly == true -> "None of its tracks are in the library."
                            else -> "Add records and tracks from their menus."
                        },
                    )
                }
            }
            itemsIndexed(lines, key = { _, line -> line.id }, contentType = { _, line -> line.contentType }) { _, line ->
                ReorderableItem(reorder, key = line.id) {
                    val isPicked = line.id in picked
                    val toggle = { picked = if (isPicked) picked - line.id else picked + line.id }
                    when (line) {
                        is PlaylistLine.Heading -> HeadingRow(
                            line, isPicked,
                            onClick = { if (selecting) toggle() else playFrom(line.positions.first()) },
                            onLongClick = { selecting = true; toggle() },
                        )
                        is PlaylistLine.Entry -> EntryRow(
                            line = line,
                            showSleeve = !grouped,
                            playing = playingEntry == line.entry.entryId,
                            favourite = line.entry.track.id in favourites,
                            picked = isPicked,
                            selecting = selecting,
                            onClick = { if (selecting) toggle() else playFrom(line.position) },
                            onLongClick = { selecting = true; toggle() },
                            onFavourite = { runtime.library.toggleFavouriteTrack(line.entry.track.id) },
                            handle = if (!fillable || selecting) null else {
                                {
                                    Box(
                                        Modifier.size(KoanShape.hit).draggableHandle(
                                            onDragStopped = {
                                                val order = PlaylistLayout.reordered(
                                                    entries, dragged ?: laidOut, line.entry.entryId,
                                                    picked.mapNotNull { id -> (laidOut.firstOrNull { it.id == id } as? PlaylistLine.Entry)?.entry?.entryId }.toSet(),
                                                )
                                                if (order == null) dragged = null else model.reorder(order, playlistId)
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
            item(key = "end", contentType = "end") { Box(Modifier.height(KoanSpace.xxl)) }
        }
    }

    if (renaming) {
        NameDialog(
            title = "rename playlist", message = null, initial = playlist?.name.orEmpty(), confirm = "rename",
            onDismiss = { renaming = false },
            onConfirm = { name -> model.rename(playlistId, name); renaming = false },
        )
    }
    if (deleting) {
        ConfirmDeleteDialog(
            PlaylistLayout.deleteQuestion(playlist),
            onDismiss = { deleting = false },
            onConfirm = {
                model.delete(playlistId)
                runtime.navigator.forget(Section.Playlist(playlistId))
                deleting = false
            },
        )
    }
    NewPlaylistHost(runtime)
    PlaylistReport(runtime)
}

private const val SETTLE_PATIENCE_MS = 1500L
private const val SYNC_LAYOUT_LIMIT = 300

/** The rows. A small playlist is laid out in place; a long one off the main thread, the previous layout up meanwhile. */
@Composable
private fun rememberLines(entries: List<PlaylistEntry>, grouped: Boolean): List<PlaylistLine> {
    if (entries.size <= SYNC_LAYOUT_LIMIT) return remember(entries, grouped) { PlaylistLayout.build(entries, grouped) }
    val state = produceState(emptyList<PlaylistLine>(), entries, grouped) {
        value = withContext(Dispatchers.Default) { PlaylistLayout.build(entries, grouped) }
    }
    return state.value
}

@Composable
private fun Header(
    name: String,
    summary: String,
    note: String?,
    cover: List<ArtworkSource>,
    hasEntries: Boolean,
    fillable: Boolean,
    renamable: Boolean,
    grouped: Boolean,
    selecting: Boolean,
    more: Boolean,
    picked: Int,
    onPlay: () -> Unit,
    onShuffle: () -> Unit,
    onPlayNext: () -> Unit,
    onQueue: () -> Unit,
    onGrouped: (Boolean) -> Unit,
    onSelect: () -> Unit,
    onMore: () -> Unit,
    onRename: () -> Unit,
    onShuffleOrder: () -> Unit,
    onDelete: () -> Unit,
    onSelectAll: () -> Unit,
    onDone: () -> Unit,
    onPickedNext: () -> Unit,
    onPickedQueue: () -> Unit,
    onPickedRemove: () -> Unit,
) {
    Column(Modifier.fillMaxWidth().padding(KoanSpace.page), verticalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
        Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.l), verticalAlignment = Alignment.Top) {
            PlaylistMosaic(cover, 96.dp)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                KoanText(name, KoanRole.TitleSm, KoanTone.Strong, data = true, maxLines = 3)
                KoanText(summary, KoanRole.Meta, KoanTone.Muted, data = true)
                if (note != null) KoanText(note, KoanRole.Fine, KoanTone.Muted, data = true, maxLines = 2)
            }
        }
        if (selecting) {
            KoanText("$picked selected", KoanRole.Body, KoanTone.Accent)
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                KoanButton("next", onPickedNext, kind = KoanButtonKind.Compact, icon = "play-next", enabled = picked > 0)
                KoanButton("queue", onPickedQueue, kind = KoanButtonKind.Compact, icon = "add-to-queue", enabled = picked > 0)
                if (fillable) KoanButton("remove", onPickedRemove, kind = KoanButtonKind.Destructive, enabled = picked > 0)
                Box(Modifier.weight(1f))
                KoanButton("all", onSelectAll, kind = KoanButtonKind.Text)
                KoanButton("done", onDone, kind = KoanButtonKind.Text)
            }
        } else {
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                KoanButton("play", onPlay, kind = KoanButtonKind.Prominent, icon = "play", enabled = hasEntries)
                KoanButton("shuffle", onShuffle, kind = KoanButtonKind.Bordered, icon = "dice", enabled = hasEntries)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                KoanButton("next", onPlayNext, kind = KoanButtonKind.Compact, icon = "play-next", enabled = hasEntries)
                KoanButton("queue", onQueue, kind = KoanButtonKind.Compact, icon = "add-to-queue", enabled = hasEntries)
                Box(Modifier.weight(1f))
                KoanIconButton(
                    "list", if (grouped) "One row per track" else "Group by album", { onGrouped(!grouped) },
                    tone = if (grouped) KoanTone.Accent else KoanTone.Muted,
                )
                KoanIconButton("select", "Select", onSelect, enabled = hasEntries)
                KoanIconButton("more", "More", onMore, tone = if (more) KoanTone.Accent else KoanTone.Ink)
            }
            if (more) {
                Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                    KoanButton("rename", onRename, kind = KoanButtonKind.Compact, enabled = renamable)
                    // Reorders the playlist itself, for good: distinct from playing it shuffled.
                    KoanButton("shuffle order", onShuffleOrder, kind = KoanButtonKind.Compact, enabled = fillable && hasEntries)
                    KoanButton("delete", onDelete, kind = KoanButtonKind.Destructive)
                }
            }
        }
    }
    KoanRule()
}

@Composable
private fun EmptyPlaylist(title: String, detail: String) {
    Column(
        Modifier.fillMaxWidth().padding(KoanSpace.page).padding(top = KoanSpace.xxl),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        KoanIcon("playlist", tone = KoanTone.Muted, textSizeDp = 22f)
        KoanText(title, KoanRole.Body, KoanTone.Strong, modifier = Modifier.padding(top = KoanSpace.m))
        KoanText(detail, KoanRole.Meta, KoanTone.Muted, modifier = Modifier.padding(top = KoanSpace.s), maxLines = 3)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun HeadingRow(line: PlaylistLine.Heading, picked: Boolean, onClick: () -> Unit, onLongClick: () -> Unit) {
    val source = line.albumId?.let { ArtworkSource.Album(it) } ?: ArtworkSource.Track(line.trackId)
    Row(
        Modifier
            .fillMaxWidth()
            .background(if (picked) KoanTheme.colors.surface else KoanTheme.colors.bg)
            .combinedClickable(role = Role.Button, onClick = onClick, onLongClick = onLongClick)
            .padding(horizontal = KoanSpace.page, vertical = KoanSpace.s),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        Sleeve(source, 44.dp)
        Column(Modifier.weight(1f)) {
            KoanText(line.album, KoanRole.Body, KoanTone.Strong, data = true)
            KoanText(line.artist.ifEmpty { "Unknown Artist" }, KoanRole.Fine, KoanTone.Muted, data = true)
        }
        if (picked) KoanIcon("check", tone = KoanTone.Accent)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun EntryRow(
    line: PlaylistLine.Entry,
    showSleeve: Boolean,
    playing: Boolean,
    favourite: Boolean,
    picked: Boolean,
    selecting: Boolean,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    onFavourite: () -> Unit,
    handle: (@Composable () -> Unit)?,
) {
    val track = line.entry.track
    Row(
        Modifier
            .fillMaxWidth()
            .background(if (picked) KoanTheme.colors.surface else KoanTheme.colors.bg)
            .combinedClickable(role = Role.Button, onClick = onClick, onLongClick = onLongClick)
            .padding(start = KoanSpace.page, top = KoanSpace.xs, bottom = KoanSpace.xs),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        Box(Modifier.width(28.dp), contentAlignment = Alignment.Center) {
            when {
                picked -> KoanIcon("check", tone = KoanTone.Accent)
                playing -> KoanIcon("play-mark", tone = KoanTone.Accent, description = "Playing")
                else -> KoanText("${line.position + 1}", KoanRole.Fine, KoanTone.Muted, data = true)
            }
        }
        if (showSleeve) Sleeve(trackSource(track), 40.dp)
        Column(Modifier.weight(1f)) {
            KoanText(track.title, KoanRole.Body, if (playing) KoanTone.Accent else KoanTone.Strong, data = true)
            KoanText(
                if (showSleeve && track.albumTitle.isNotEmpty()) "${track.artistName} · ${track.albumTitle}" else track.artistName,
                KoanRole.Fine, KoanTone.Muted, data = true,
            )
        }
        KoanText(track.durationMs?.let { PlaylistLayout.duration(it) } ?: "--:--", KoanRole.Fine, KoanTone.Muted, data = true)
        if (!selecting) {
            KoanIconButton(
                if (favourite) "heart-filled" else "heart",
                if (favourite) "Remove favourite" else "Favourite",
                onFavourite,
                tone = if (favourite) KoanTone.Accent else KoanTone.Muted,
            )
        }
        if (handle != null) handle() else if (!selecting) Box(Modifier.width(KoanSpace.xs))
    }
}

private fun trackSource(track: Track): ArtworkSource =
    track.albumId?.let { ArtworkSource.Album(it) } ?: ArtworkSource.Track(track.id)

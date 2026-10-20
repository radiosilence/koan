package cc.blit.koan.ui.playlists

import android.graphics.Bitmap
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.art.Art
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ffi.PlaylistEntry
import cc.blit.koan.ui.downloads.DownloadMenuItems
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.library.PageBar
import cc.blit.koan.ui.library.PageHeader
import cc.blit.koan.ui.library.Playable
import cc.blit.koan.ui.library.Removal
import cc.blit.koan.ui.library.SelectButton
import cc.blit.koan.ui.library.SelectMode
import cc.blit.koan.ui.library.Selection
import cc.blit.koan.ui.library.SelectionBar
import cc.blit.koan.ui.library.Sleeve
import cc.blit.koan.ui.library.Tick
import cc.blit.koan.ui.library.albumName
import cc.blit.koan.ui.library.GoToArtists
import cc.blit.koan.ui.library.artistCredit
import cc.blit.koan.ui.library.favourites
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.nowplaying.LocalShowTrackInfo
import cc.blit.koan.ui.nowplaying.clock
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIconToggle
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanRecordAccent
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap

/**
 * A playlist, laid out the way the queue is: grouped under its records or one
 * row per track, the choice remembered per playlist and ungrouped by default,
 * since a playlist is a sequence someone chose rather than a shelf of records.
 *
 * Playback is mirrored onto it: a row is lit because its entry is what is
 * playing. Editing (drag to reorder, remove) is for playlists that take it; a
 * smart playlist's rules and a playlist file decide their own contents.
 */
@Composable
fun PlaylistPage(playlistId: Long) {
    val app = koanApp()
    val model = viewModel { PlaylistModel(app.mirror, playlistId) }
    val contents by model.contents.collectAsStateWithLifecycle()
    val nowPlaying by app.mirror.nowPlaying.collectAsStateWithLifecycle()
    val edits = remember { PlaylistEdits(app.mirror) }
    val nav = LocalNavigator.current

    val playlist = contents?.playlist
    val entries = contents?.entries.orEmpty()
    val fillable = playlist?.fillable == true
    var editing by rememberSaveable { mutableStateOf(false) }
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }
    var adding by remember { mutableStateOf<List<Long>?>(null) }
    val allTrackIds: suspend KoanEngine.() -> List<Long> = { playlistTracks(playlistId).map { it.track.id } }
    // Entries, not tracks: the same song twice is two rows to pick.
    val selection = remember { Selection<PlaylistEntry> { it.entryId } }
    SelectMode(selection)
    val list = rememberLazyListState()
    // The name moves into the bar once the header has scrolled away.
    val scrolled by remember { derivedStateOf { list.firstVisibleItemIndex > 0 } }

    PlaylistAccent(contents?.covers?.firstOrNull()) {
        Column {
            val title = when {
                playlist == null -> if (contents == null) "" else "Playlist"
                scrolled -> playlist.name
                else -> ""
            }
            PageBar(title, appText = playlist == null) {
                if (entries.isNotEmpty() && !editing) SelectButton(selection)
                if (fillable && entries.isNotEmpty() && !selection.active) {
                    KoanButton(if (editing) "Done" else "Edit", { editing = !editing }, kind = KoanButtonKind.Text)
                }
                Box {
                    KoanButton("More", { menu = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.more)
                    KoanMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        if (playlist?.fromFile == false) KoanMenuItem("Rename…") { menu = false; renaming = true }
                        if (fillable && entries.size > 1) {
                            KoanMenuItem("Shuffle Order", KoanIcons.reshuffle) { menu = false; edits.shuffleOrder(playlistId) }
                        }
                        if (entries.isNotEmpty()) {
                            KoanMenuItem("Add to Playlist…", KoanIcons.playlist) { menu = false; adding = entries.map { it.track.id } }
                            DownloadMenuItems(onDisk = null, dismiss = { menu = false }, trackIds = allTrackIds)
                        }
                        KoanMenuItem("Delete Playlist", KoanIcons.clear, destructive = true) { menu = false; deleting = true }
                    }
                }
            }

            Box(Modifier.weight(1f)) {
                when {
                    contents == null -> Unit
                    playlist == null -> KoanUnavailable("Playlist not found", KoanIcons.playlist, "It may have been deleted here or on your server.")
                    else -> PlaylistList(
                        listState = list,
                        playlist = playlist,
                        entries = entries,
                        covers = contents?.covers.orEmpty(),
                        playingEntry = nowPlaying?.entry?.playlistEntryId,
                        editing = editing && fillable,
                        selection = selection,
                        edits = edits,
                        onAdd = { adding = it },
                    )
                }
            }
            if (selection.active) {
                SelectionBar(
                    selection,
                    { Playable.OfTrack(it.track) },
                    if (fillable) Removal("Remove") { picked -> edits.remove(playlistId, picked.map { it.entryId }) } else null,
                )
            }
        }
    }

    adding?.let { ids -> AddToPlaylistSheet({ ids }) { adding = null } }
    if (renaming && playlist != null) {
        NameDialog("Rename Playlist", "Rename", playlist.name, onDismiss = { renaming = false }) {
            renaming = false
            edits.rename(playlist, it)
        }
    }
    if (deleting && playlist != null) {
        DeletePlaylistDialog(playlist, onDismiss = { deleting = false }) {
            edits.delete(playlistId)
            nav.pop()
        }
    }
}

@Composable
private fun PlaylistList(
    listState: LazyListState,
    playlist: Playlist,
    entries: List<PlaylistEntry>,
    covers: List<Long>,
    playingEntry: Long?,
    editing: Boolean,
    selection: Selection<PlaylistEntry>,
    edits: PlaylistEdits,
    onAdd: (List<Long>) -> Unit,
) {
    val grouped = playlist.grouped ?: false
    // One state object for the page's life: each row's gesture outlives a
    // recomposition and keeps the state it started with.
    val latest by rememberUpdatedState(entries)
    var order by remember { mutableStateOf(entries) }
    var dragging by remember { mutableStateOf<Long?>(null) }
    var offset by remember { mutableFloatStateOf(0f) }
    // The engine's order, except under a drag: a library update landing then
    // must not move rows out from under the finger. The drop is written and comes back.
    LaunchedEffect(entries) { if (dragging == null) order = entries }

    fun dragBy(delta: Float) {
        offset += delta
        val visible = listState.layoutInfo.visibleItemsInfo
        val current = visible.firstOrNull { it.key == "entry:$dragging" } ?: return
        val middle = current.offset + offset + current.size / 2
        val target = visible.firstOrNull {
            it.key != current.key && (it.key as? String)?.startsWith("entry:") == true &&
                middle >= it.offset && middle < it.offset + it.size
        } ?: return
        val from = order.indexOfFirst { it.entryId == dragging }
        val to = order.indexOfFirst { "entry:${it.entryId}" == target.key }
        if (from < 0 || to < 0) return
        order = order.toMutableList().apply { add(to, removeAt(from)) }
        // The row is laid out at its new place next frame; keep it under the finger.
        offset += current.offset - target.offset
    }

    // The move is applied to the entries as they are now, not as they were
    // when the drag began: the dragged row goes after the row it was dropped
    // below, so anything added, removed or moved meanwhile is kept.
    fun drop(commit: Boolean) {
        val moved = dragging
        val shown = order
        dragging = null
        offset = 0f
        order = latest
        if (!commit || moved == null) return
        val at = shown.indexOfFirst { it.entryId == moved }
        val entry = latest.firstOrNull { it.entryId == moved } ?: return
        val above = shown.take(at.coerceAtLeast(0)).lastOrNull { prior -> latest.any { it.entryId == prior.entryId } }
        val rest = latest.filter { it.entryId != moved }
        val arranged = rest.toMutableList().apply {
            add(if (above == null) 0 else rest.indexOfFirst { it.entryId == above.entryId } + 1, entry)
        }
        if (arranged.map { it.entryId } == latest.map { it.entryId }) return
        order = arranged
        // A failed write brings no new entries, so nothing else would undo the move shown.
        edits.reorder(playlist.id, arranged.map { it.entryId }) { if (dragging == null) order = latest }
    }

    // Near an edge, a dragged row scrolls the list a frame at a time, and the
    // list moving under it is a drag like any other.
    val edge = with(LocalDensity.current) { 64.dp.toPx() }
    LaunchedEffect(dragging) {
        if (dragging == null) return@LaunchedEffect
        while (true) {
            withFrameNanos {}
            val info = listState.layoutInfo
            val current = info.visibleItemsInfo.firstOrNull { it.key == "entry:$dragging" } ?: continue
            val top = current.offset + offset
            val step = edgeScroll(top, top + current.size, info.viewportStartOffset.toFloat(), info.viewportEndOffset.toFloat(), edge)
            if (step == 0f) continue
            val moved = listState.scrollBy(step)
            offset += moved
            dragBy(0f)
        }
    }

    val rows = if (editing) order.map(PlaylistRow::Entry) else playlistRows(entries, grouped)
    LazyColumn(state = listState) {
        item(key = "header") {
            PlaylistHeader(playlist, entries, covers, grouped, showLayout = !editing, edits)
        }
        if (entries.isEmpty()) {
            item(key = "empty") {
                KoanUnavailable(
                    "Nothing in here yet",
                    KoanIcons.playlist,
                    if (playlist.fillable) "Add tracks from a record, a search or the queue." else null,
                    Modifier.height(200.dp),
                )
            }
        }
        items(rows, key = { it.key }) { row ->
            when (row) {
                is PlaylistRow.Heading -> RecordHeading(row, selection)
                is PlaylistRow.Entry -> {
                    val id = row.entry.entryId
                    EntryRow(
                        entry = row.entry,
                        playlist = playlist,
                        artwork = !grouped || editing,
                        playing = playingEntry == id,
                        editing = editing,
                        selection = selection,
                        edits = edits,
                        onAdd = onAdd,
                        handle = Modifier.pointerInput(id) {
                            detectDragGestures(
                                onDragStart = { dragging = id; offset = 0f },
                                onDragEnd = { drop(commit = true) },
                                onDragCancel = { drop(commit = false) },
                                onDrag = { change, amount -> change.consume(); dragBy(amount.y) },
                            )
                        },
                        modifier = if (dragging == id) {
                            Modifier.zIndex(1f).graphicsLayer { translationY = offset }
                        } else {
                            Modifier.animateItem()
                        },
                    )
                }
            }
        }
    }
}

@Composable
private fun PlaylistHeader(
    playlist: Playlist,
    entries: List<PlaylistEntry>,
    covers: List<Long>,
    grouped: Boolean,
    showLayout: Boolean,
    edits: PlaylistEdits,
) {
    val app = koanApp()
    val any = entries.isNotEmpty()
    PageHeader(
        playlist.name,
        Modifier.padding(horizontal = LocalKoanTokens.current.gutter, vertical = KoanSpace.s),
        art = { PlaylistMosaic(covers, 112.dp) },
        detail = { KoanText(contentsSummary(playlist, entries), role = KoanType.Meta, tone = KoanTone.Muted) },
        actions = {
            KoanButton("Play", { edits.play(playlist.id) }, kind = KoanButtonKind.Prominent, icon = KoanIcons.play, enabled = any)
            KoanButton("Shuffle", { edits.play(playlist.id, shuffled = true) }, icon = KoanIcons.shuffle, enabled = any)
        },
        below = {
            // The lesser verbs on the whole playlist, compact, under its one Play.
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
                KoanButton("Play Next", { app.player.playNext(entries.map { it.track.id }) }, kind = KoanButtonKind.Compact, icon = KoanIcons.playNext, enabled = any)
                KoanButton("Add to Queue", { app.player.enqueue(entries.map { it.track.id }) }, kind = KoanButtonKind.Compact, icon = KoanIcons.queue, enabled = any)
                Spacer(Modifier.weight(1f))
                if (showLayout && any) KoanIconToggle(KoanIcons.album, "Group by album", grouped) { edits.setGrouped(playlist.id, it) }
            }
        },
    )
}

/**
 * How far to scroll this frame for a row dragged between `top` and `bottom`:
 * nothing away from the edges, and faster the further into the `edge` band it
 * reaches, up to a fifth of the band a frame.
 */
fun edgeScroll(top: Float, bottom: Float, start: Float, end: Float, edge: Float): Float = when {
    top < start + edge -> -((start + edge - top).coerceAtMost(edge) / 5f)
    bottom > end - edge -> (bottom - (end - edge)).coerceAtMost(edge) / 5f
    else -> 0f
}

/**
 * The page in the colour of the playlist's first sleeve, as a record's page is
 * in its own.
 */
@Composable
private fun PlaylistAccent(albumId: Long?, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val sleeve by produceState<Bitmap?>(null, albumId) {
        value = albumId?.let {
            val request = ImageRequest.Builder(context).data(Art.album(it)).allowHardware(false).build()
            (SingletonImageLoader.get(context).execute(request) as? SuccessResult)?.image?.toBitmap()
        }
    }
    KoanRecordAccent(sleeve, content)
}

/** "12 tracks · 48 min · synced": what it holds, and whether edits reach the server. */
private fun contentsSummary(playlist: Playlist, entries: List<PlaylistEntry>): String {
    val parts = mutableListOf(trackCount(entries.size.toUInt()))
    val total = entries.sumOf { it.track.durationMs ?: 0L }
    if (total > 0) parts += duration(total)
    if (playlist.remoteId != null) parts += "synced"
    when {
        playlist.smart -> parts += "smart"
        playlist.fromFile -> parts += "from a file"
        playlist.readonly -> parts += "read-only"
    }
    return parts.joinToString(" · ")
}

@Composable
private fun RecordHeading(heading: PlaylistRow.Heading, selection: Selection<PlaylistEntry>) {
    val nav = LocalNavigator.current
    CollectionRow(
        heading.album,
        null,
        names = listOf(heading.entries.first().let { artistCredit(it.albumArtistCredit, heading.artist, it.albumArtistId) }),
        linked = !selection.active,
        modifier = Modifier.padding(top = KoanSpace.m),
        titleTone = KoanTone.Strong,
        // While picking, a heading picks its run, or lets it go when the whole run is picked.
        onClick = when {
            selection.active -> ({ pickRun(selection, heading.entries) })
            else -> heading.albumId?.let { id -> { nav.push(AlbumRoute(id)) } }
        },
        leading = heading.albumId?.let { id -> { Sleeve(Art.album(id)) } },
    )
}

/** Tick every entry of a run, or untick them all when every one is ticked already. */
fun pickRun(selection: Selection<PlaylistEntry>, run: List<PlaylistEntry>) {
    val all = run.all(selection::contains)
    run.filter { selection.contains(it) == all }.forEach { selection.take(it) }
}

@Composable
private fun EntryRow(
    entry: PlaylistEntry,
    playlist: Playlist,
    artwork: Boolean,
    playing: Boolean,
    editing: Boolean,
    selection: Selection<PlaylistEntry>,
    edits: PlaylistEdits,
    onAdd: (List<Long>) -> Unit,
    modifier: Modifier,
    handle: Modifier,
) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val track = entry.track
    var menu by remember { mutableStateOf(false) }
    val showInfo = LocalShowTrackInfo.current

    Box(modifier) {
        CollectionRow(
            track.title,
            null,
            // Under a heading the record is named already, and so is its artist.
            names = listOfNotNull(
                artistCredit(track.artistCredit, track.artistName, track.artistId).takeIf { artwork || track.artistName != track.albumArtistName },
                albumName(track.albumTitle, track.albumId, track.id).takeIf { artwork },
            ),
            linked = !selection.active && !editing,
            titleTone = if (playing) KoanTone.Accent else KoanTone.Ink,
            selected = playing,
            onClick = if (editing) null else ({ if (!selection.take(entry)) edits.play(playlist.id, entry.entryId) }),
            onLongClick = if (editing || selection.active) null else ({ menu = true }),
            leading = {
                when {
                    selection.active -> Tick(selection.contains(entry))
                    editing -> KoanButton("Remove", { edits.remove(playlist.id, listOf(entry.entryId)) }, kind = KoanButtonKind.Icon, icon = KoanIcons.subtract, destructive = true)
                    artwork -> Sleeve(track.albumId?.let { Art.album(it) } ?: Art.track(track.id))
                    else -> KoanText(track.trackNumber?.toString().orEmpty(), Modifier.width(24.dp), role = KoanType.Meta, tone = KoanTone.Muted)
                }
            },
            trailing = {
                if (editing) {
                    KoanIcon(KoanIcons.choose, "Reorder", handle.padding(KoanSpace.m))
                } else {
                    KoanText(track.durationMs?.let(::clock).orEmpty(), role = KoanType.Meta, tone = KoanTone.Muted)
                }
            },
        )
        KoanMenu(expanded = menu, onDismissRequest = { menu = false }) {
            KoanMenuItem("Play", KoanIcons.play) { menu = false; edits.play(playlist.id, entry.entryId) }
            KoanMenuItem("Play Next", KoanIcons.playNext) { menu = false; app.player.playNext(listOf(track.id)) }
            KoanMenuItem("Add to Queue", KoanIcons.queue) { menu = false; app.player.enqueue(listOf(track.id)) }
            KoanMenuItem("Add to Playlist…", KoanIcons.playlist) { menu = false; onAdd(listOf(track.id)) }
            // Read here, in the menu, so a favourite toggled elsewhere recomposes no closed rows.
            val store = favourites()
            val favourite = track.id in store.tracks.collectAsStateWithLifecycle().value
            KoanMenuItem(if (favourite) "Remove Favourite" else "Favourite Track", if (favourite) KoanIcons.favourited else KoanIcons.favourite) {
                menu = false
                store.toggleTrack(track.id)
            }
            DownloadMenuItems(onDisk = track.onDisk, dismiss = { menu = false }) { listOf(track.id) }
            track.albumId?.let { albumId ->
                KoanMenuItem("Go to Album", KoanIcons.album) { menu = false; nav.push(AlbumRoute(albumId, track.id)) }
            }
            GoToArtists(track.artistCredit, track.artistId) { menu = false }
            KoanMenuItem("Info…", KoanIcons.info) { menu = false; showInfo(track.id) }
            if (playlist.fillable) {
                KoanMenuItem("Remove from Playlist", KoanIcons.remove, destructive = true) {
                    menu = false
                    edits.remove(playlist.id, listOf(entry.entryId))
                }
            }
        }
    }
}

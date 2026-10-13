package cc.blit.koan.ui.playlists

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.R
import cc.blit.koan.art.Art
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ffi.PlaylistEntry
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.downloads.DownloadMenuItems
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.nowplaying.clock
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage

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
    val allTrackIds: suspend KoanEngine.() -> List<Long> =
        { playlistTracks(playlistId).map { it.track.id } }

    Column {
        PageBar(playlist?.name ?: "Playlist") {
            if (fillable && entries.isNotEmpty()) {
                IconButton(onClick = { editing = !editing }) {
                    Icon(painterResource(if (editing) R.drawable.koan_check else R.drawable.koan_chevron_up_down), if (editing) "Done" else "Edit")
                }
            }
            Box {
                IconButton(onClick = { menu = true }) { Icon(painterResource(R.drawable.koan_more), "More") }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    if (playlist?.fromFile == false) {
                        DropdownMenuItem(text = { Text("Rename…") }, onClick = { menu = false; renaming = true })
                    }
                    if (fillable && entries.size > 1) {
                        DropdownMenuItem(
                            text = { Text("Shuffle Order") },
                            leadingIcon = { Icon(painterResource(R.drawable.koan_shuffle), null) },
                            onClick = { menu = false; edits.shuffleOrder(playlistId) },
                        )
                    }
                    if (entries.isNotEmpty()) {
                        DropdownMenuItem(
                            text = { Text("Add to Playlist…") },
                            leadingIcon = { Icon(painterResource(R.drawable.koan_playlist), null) },
                            onClick = { menu = false; adding = entries.map { it.track.id } },
                        )
                        DownloadMenuItems(onDisk = null, dismiss = { menu = false }, trackIds = allTrackIds)
                    }
                    DropdownMenuItem(text = { Text("Delete Playlist") }, onClick = { menu = false; deleting = true })
                }
            }
        }

        when {
            contents == null -> Unit
            playlist == null -> StubPage("Playlist not found")
            else -> PlaylistList(
                playlist = playlist,
                entries = entries,
                covers = contents?.covers.orEmpty(),
                playingEntry = nowPlaying?.entry?.playlistEntryId,
                editing = editing && fillable,
                edits = edits,
                onAdd = { adding = it },
            )
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
        ConfirmDialog(
            "Delete “${playlist.name}”?",
            if (playlist.remoteId != null) "It is deleted on your server too." else "This cannot be undone.",
            "Delete",
            onDismiss = { deleting = false },
        ) {
            edits.delete(playlistId)
            nav.pop()
        }
    }
}

@Composable
private fun PlaylistList(
    playlist: Playlist,
    entries: List<PlaylistEntry>,
    covers: List<Long>,
    playingEntry: Long?,
    editing: Boolean,
    edits: PlaylistEdits,
    onAdd: (List<Long>) -> Unit,
) {
    val grouped = playlist.grouped ?: false
    val listState = rememberLazyListState()
    // The order shown while a row is dragged; the engine's again once the drop lands and the library moves.
    var order by remember(entries) { mutableStateOf(entries) }
    var dragging by remember { mutableStateOf<Long?>(null) }
    var offset by remember { mutableFloatStateOf(0f) }

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

    fun drop() {
        if (dragging != null && order.map { it.entryId } != entries.map { it.entryId }) {
            edits.reorder(playlist.id, order.map { it.entryId })
        }
        dragging = null
        offset = 0f
    }

    val rows = if (editing) order.map(PlaylistRow::Entry) else playlistRows(entries, grouped)
    LazyColumn(state = listState) {
        item(key = "header") {
            PlaylistHeader(playlist, entries, covers, grouped, showLayout = !editing, edits)
        }
        if (entries.isEmpty()) {
            item(key = "empty") { StubPage("Nothing in here yet", Modifier.height(160.dp)) }
        }
        items(rows, key = { it.key }) { row ->
            when (row) {
                is PlaylistRow.Heading -> RecordHeading(row)
                is PlaylistRow.Entry -> {
                    val id = row.entry.entryId
                    val lifted = dragging == id
                    EntryRow(
                        entry = row.entry,
                        playlist = playlist,
                        artwork = !grouped || editing,
                        playing = playingEntry == id,
                        editing = editing,
                        edits = edits,
                        onAdd = onAdd,
                        handle = Modifier.pointerInput(id) {
                            detectDragGestures(
                                onDragStart = { dragging = id; offset = 0f },
                                onDragEnd = { drop() },
                                onDragCancel = { drop() },
                                onDrag = { change, amount -> change.consume(); dragBy(amount.y) },
                            )
                        },
                        modifier = if (lifted) {
                            Modifier.zIndex(1f).graphicsLayer { translationY = offset; shadowElevation = 8f }
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
    val gutter = LocalKoanTokens.current.gutter
    Column(Modifier.padding(horizontal = gutter, vertical = LocalKoanTokens.current.rowGap)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            PlaylistMosaic(covers, 112.dp)
            Spacer(Modifier.width(gutter))
            Column {
                Text(playlist.name, style = MaterialTheme.typography.titleLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(contentsSummary(playlist, entries), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        Spacer(Modifier.height(gutter))
        Row(horizontalArrangement = Arrangement.spacedBy(LocalKoanTokens.current.rowGap), verticalAlignment = Alignment.CenterVertically) {
            Button(onClick = { edits.play(playlist.id) }, enabled = entries.isNotEmpty()) {
                Icon(painterResource(R.drawable.ic_play), null, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text("Play")
            }
            FilledTonalButton(onClick = { edits.play(playlist.id, shuffled = true) }, enabled = entries.isNotEmpty()) {
                Icon(painterResource(R.drawable.koan_shuffle), null, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text("Shuffle")
            }
            Spacer(Modifier.weight(1f))
            if (showLayout && entries.isNotEmpty()) {
                FilterChip(
                    selected = grouped,
                    onClick = { edits.setGrouped(playlist.id, !grouped) },
                    label = { Text("By album") },
                    leadingIcon = { Icon(painterResource(R.drawable.koan_album), null, Modifier.size(18.dp)) },
                )
            }
        }
    }
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
private fun RecordHeading(heading: PlaylistRow.Heading) {
    val nav = LocalNavigator.current
    val gutter = LocalKoanTokens.current.gutter
    Column(
        Modifier
            .clickable(enabled = heading.albumId != null) { heading.albumId?.let { nav.push(AlbumRoute(it)) } }
            .padding(start = gutter, end = gutter, top = gutter, bottom = 4.dp),
    ) {
        HorizontalDivider(Modifier.padding(bottom = 8.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            heading.albumId?.let {
                AsyncImage(Art.album(it), null, Modifier.size(LocalKoanTokens.current.thumb).padding(end = 12.dp))
            }
            Column {
                Text(heading.album, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(heading.artist, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun EntryRow(
    entry: PlaylistEntry,
    playlist: Playlist,
    artwork: Boolean,
    playing: Boolean,
    editing: Boolean,
    edits: PlaylistEdits,
    onAdd: (List<Long>) -> Unit,
    handle: Modifier,
    modifier: Modifier,
) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val track = entry.track
    var menu by remember { mutableStateOf(false) }
    val accent = if (playing) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface

    Box(modifier) {
        ListItem(
            headlineContent = { Text(track.title, color = accent, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            supportingContent = { Text(track.artistName, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            leadingContent = {
                when {
                    editing -> IconButton(onClick = { edits.remove(playlist.id, listOf(entry.entryId)) }) {
                        Icon(painterResource(R.drawable.koan_minus), "Remove")
                    }
                    artwork -> AsyncImage(track.albumId?.let { Art.album(it) } ?: Art.track(track.id), null, Modifier.size(LocalKoanTokens.current.thumb))
                    else -> Text(track.trackNumber?.toString().orEmpty(), color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.width(24.dp))
                }
            },
            trailingContent = {
                if (editing) {
                    Icon(painterResource(R.drawable.koan_chevron_up_down), "Reorder", handle.padding(8.dp))
                } else {
                    Text(track.durationMs?.let(::clock).orEmpty(), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            },
            modifier = Modifier.combinedClickable(
                enabled = !editing,
                onClick = { edits.play(playlist.id, entry.entryId) },
                onLongClick = { menu = true },
            ),
        )
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            DropdownMenuItem(text = { Text("Play") }, onClick = { menu = false; edits.play(playlist.id, entry.entryId) })
            DropdownMenuItem(
                text = { Text("Play Next") },
                leadingIcon = { Icon(painterResource(R.drawable.koan_play_next), null) },
                onClick = { menu = false; app.player.playNext(listOf(track.id)) },
            )
            DropdownMenuItem(
                text = { Text("Add to Queue") },
                leadingIcon = { Icon(painterResource(R.drawable.koan_add_to_queue), null) },
                onClick = { menu = false; app.player.enqueue(listOf(track.id)) },
            )
            DropdownMenuItem(
                text = { Text("Add to Playlist…") },
                leadingIcon = { Icon(painterResource(R.drawable.koan_playlist), null) },
                onClick = { menu = false; onAdd(listOf(track.id)) },
            )
            DownloadMenuItems(onDisk = track.onDisk, dismiss = { menu = false }) { listOf(track.id) }
            track.albumId?.let { albumId ->
                DropdownMenuItem(
                    text = { Text("Go to Album") },
                    leadingIcon = { Icon(painterResource(R.drawable.koan_album), null) },
                    onClick = { menu = false; nav.push(AlbumRoute(albumId)) },
                )
            }
            if (playlist.fillable) {
                DropdownMenuItem(
                    text = { Text("Remove from Playlist") },
                    leadingIcon = { Icon(painterResource(R.drawable.koan_minus), null) },
                    onClick = { menu = false; edits.remove(playlist.id, listOf(entry.entryId)) },
                )
            }
        }
    }
}

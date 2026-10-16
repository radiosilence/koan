package cc.blit.koan.ui.pages

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.clickable
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.engine.EngineDispatcher
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withContext

/**
 * One record. Its tracks are in the record the library read before the
 * navigator moved here, so the first frame already has the whole page; a
 * record that failed to read or has gone says so, with the ways out. See
 * `AlbumDetailView` and `TrackListView` in the Swift app, which this follows.
 *
 * Tap plays from that track; long-press starts a pick. Arriving from search
 * scrolls to the matched track and marks it until the next tap.
 */
@Composable
fun AlbumPage(runtime: KoanRuntime, albumId: Long) {
    val library = runtime.library
    val navigator = runtime.navigator
    val records by library.detailRecords.collectAsStateWithLifecycle()
    val libraryVersion by runtime.mirror.libraryVersion.collectAsStateWithLifecycle()
    var retry by remember { mutableIntStateOf(0) }

    // The record arrived before the page did. Asked again for a library change
    // (a download landing writes a path onto one of these rows) or a retry; a
    // record in hand and unchanged answers at once. Run on the engine's
    // dispatcher, where the library keeps its state.
    LaunchedEffect(albumId, libraryVersion, retry) {
        withContext(EngineDispatcher.dispatcher) { library.prepareAlbum(albumId) }
    }

    when (val view = albumView(records[albumId])) {
        AlbumView.Loading -> Box(Modifier.fillMaxSize())
        AlbumView.Failed -> AlbumMessage("Couldn't read this album", runtime, onRetry = { retry++ })
        AlbumView.Gone -> AlbumMessage("This album isn't in the library any more", runtime, onRetry = null)
        is AlbumView.Ready -> AlbumContent(runtime, view)
    }
}

@Composable
private fun AlbumMessage(text: String, runtime: KoanRuntime, onRetry: (() -> Unit)?) {
    val nav by runtime.navigator.state.collectAsStateWithLifecycle()
    val actions = buildList {
        if (nav.canPop) add(BarAction("Go back", { runtime.navigator.back() }))
        if (onRetry != null) add(BarAction("Try again", onRetry))
    }
    RecordMessage(text, actions)
}

@Composable
private fun AlbumContent(runtime: KoanRuntime, view: AlbumView.Ready) {
    val album = view.album
    val tracks = view.tracks
    val lines = remember(tracks) { trackLines(tracks) }
    val ids = remember(tracks) { tracks.map { it.id } }
    val actions = rememberRecordActions(runtime)
    val navigator = runtime.navigator
    val mirror = runtime.mirror
    val favourites by runtime.library.favouriteTrackIds.collectAsStateWithLifecycle()
    val queued by mirror.queueIndex.collectAsStateWithLifecycle()
    // Two small answers rather than the whole now-playing value: the page
    // recomposes when the track changes or play state flips, not on each batch.
    val current by remember(mirror) { mirror.playback.map { it.entry?.trackId }.distinctUntilChanged() }
        .collectAsStateWithLifecycle(mirror.playback.value.entry?.trackId)
    val highlightFromNav by navigator.highlightedTrackId.collectAsStateWithLifecycle()
    var pick by remember { mutableStateOf(Pick<Long>()) }
    // The matched track stays marked until the next tap; the navigator's own
    // value is spent as soon as it has been scrolled to.
    var highlighted by remember { mutableStateOf<Long?>(null) }
    val listState = rememberLazyListState()
    BackHandler(enabled = pick.active) { pick = pick.end() }

    LaunchedEffect(highlightFromNav, lines) {
        val target = highlightFromNav ?: return@LaunchedEffect
        val index = highlightIndex(lines, target) ?: return@LaunchedEffect
        highlighted = target
        listState.animateScrollToItem(index)
        navigator.clearHighlight()
    }

    val numberWidth = remember(tracks.size) { numberColumn(tracks.size) }
    val statusTop = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val picked = pick.picked
    val chosen: List<Long> = remember(picked, ids) { ids.toSet().let { held -> picked.filter { it in held } } }

    BoxWithConstraints(Modifier.fillMaxSize().background(KoanTheme.colors.bg)) {
        val columns = gridColumns(maxWidth)
        LazyColumn(
            Modifier.fillMaxSize(),
            state = listState,
            contentPadding = PaddingValues(top = statusTop + KoanSpace.l, bottom = if (pick.active) 160.dp else KoanSpace.xxl),
        ) {
            item(key = "header", contentType = "header") {
                AlbumHeader(runtime, album.id, album.title, album.artistId, album.artistName, albumDetails(album, tracks), columns, ids, actions, pick) { pick = it }
            }
            items(lines, key = { it.key }, contentType = { if (it is TrackLine.Disc) "disc" else "track" }) { line ->
                when (line) {
                    is TrackLine.Disc -> DiscHeading(line.disc)
                    is TrackLine.Row -> {
                        val track = line.track
                        TrackRowView(
                            track = track,
                            position = line.position + 1,
                            numberWidth = numberWidth,
                            showsAlbum = false,
                            state = RowState(
                                current = current == track.id,
                                favourite = track.id in favourites,
                                availability = availability(track, queued.byTrack[track.id]),
                                selecting = pick.active,
                                picked = track.id in pick,
                                highlighted = highlighted == track.id,
                            ),
                            onTap = {
                                highlighted = null
                                if (pick.active) pick = pick.toggle(track.id) else actions.play(ids, line.position)
                            },
                            onLongPress = { highlighted = null; pick = if (pick.active) pick.toggle(track.id) else pick.begin(track.id) },
                            onHeart = { actions.toggleFavouriteTrack(track.id) },
                        )
                    }
                }
            }
        }
        if (pick.active) {
            SelectionBar(
                pick.count,
                listOf(
                    BarAction("Play", { actions.play(chosen); pick = pick.end() }, chosen.isNotEmpty()),
                    BarAction("Play next", { actions.playNext(chosen); pick = pick.end() }, chosen.isNotEmpty()),
                    BarAction("Add to queue", { actions.enqueue(chosen); pick = pick.end() }, chosen.isNotEmpty()),
                    BarAction("Download", { actions.download(chosen); pick = pick.end() }, chosen.isNotEmpty()),
                    BarAction("Remove downloads", { actions.removeDownloads(chosen); pick = pick.end() }, chosen.isNotEmpty()),
                    BarAction("All", { pick = pick.selectAll(ids) }),
                    BarAction("Done", { pick = pick.end() }),
                ),
                Modifier.align(Alignment.BottomCenter),
            )
        }
    }
}

@Composable
private fun AlbumHeader(
    runtime: KoanRuntime,
    albumId: Long,
    title: String,
    artistId: Long,
    artistName: String,
    details: String,
    columns: Int,
    ids: List<Long>,
    actions: RecordActions,
    pick: Pick<Long>,
    onPick: (Pick<Long>) -> Unit,
) {
    val favouriteAlbums by runtime.library.favouriteAlbumIds.collectAsStateWithLifecycle()
    val favourite = albumId in favouriteAlbums
    RecordHeader(
        title = title,
        columns = columns,
        cover = { size -> Sleeve(ArtworkSource.Album(albumId), size, description = "Cover of $title") },
        subtitle = {
            KoanText(
                artistName, KoanRole.Control, KoanTone.Ink, data = true, maxLines = 2,
                modifier = Modifier.clickable { runtime.navigator.openArtist(artistId) },
            )
            if (details.isNotEmpty()) KoanText(details, KoanRole.Fine, KoanTone.Muted, data = true, maxLines = 2)
        },
        actions = {
            KoanButton("Play", { actions.play(ids) }, kind = KoanButtonKind.Prominent, enabled = ids.isNotEmpty())
            KoanButton("Shuffle", { actions.playAlbum(albumId, title, shuffled = true) }, kind = KoanButtonKind.Bordered, enabled = ids.isNotEmpty())
            KoanButton("Play next", { actions.playNext(ids) }, kind = KoanButtonKind.Compact)
            KoanButton("Add to queue", { actions.enqueue(ids) }, kind = KoanButtonKind.Compact)
            KoanButton("Download", { actions.download(ids) }, kind = KoanButtonKind.Compact)
            KoanButton("Remove downloads", { actions.removeDownloads(ids) }, kind = KoanButtonKind.Compact)
            KoanButton("Select", { onPick(if (pick.active) pick.end() else pick.begin()) }, kind = KoanButtonKind.Compact)
            KoanIconButton(
                icon = if (favourite) "heart-filled" else "heart",
                description = if (favourite) "Unfavourite $title" else "Favourite $title",
                onClick = { runtime.library.toggleFavouriteAlbum(albumId) },
                tone = if (favourite) KoanTone.Accent else KoanTone.Muted,
            )
        },
    )
}

package cc.blit.koan.ui.pages

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.engine.EngineDispatcher
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
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
import uniffi.koan_ffi.Album

/**
 * One artist: a photograph and a name, what you can do with them, the
 * biography (cut to a few lines, a tap gives the rest) and a grid of their
 * records. An artist credited only on other people's records gets those
 * tracks instead of an empty grid. See `ArtistDetailView` in the Swift app.
 *
 * The photograph and the biography are fetched from the network after the
 * page is up; the page draws from what is cached and grows into the rest.
 */
@Composable
fun ArtistPage(runtime: KoanRuntime, artistId: Long) {
    val library = runtime.library
    val record by library.detailArtist.collectAsStateWithLifecycle()
    val libraryVersion by runtime.mirror.libraryVersion.collectAsStateWithLifecycle()
    var retry by remember { mutableIntStateOf(0) }

    // The record arrived before the page did; asked again for a library change.
    LaunchedEffect(artistId, libraryVersion, retry) {
        withContext(EngineDispatcher.dispatcher) { library.prepareArtist(artistId) }
    }

    when (val view = artistView(record, artistId)) {
        ArtistView.Loading -> Box(Modifier.fillMaxSize())
        ArtistView.Missing -> {
            val nav by runtime.navigator.state.collectAsStateWithLifecycle()
            RecordMessage(
                "This artist isn't in the library any more",
                buildList {
                    if (nav.canPop) add(BarAction("Go back", { runtime.navigator.back() }))
                    add(BarAction("Try again", { retry++ }))
                },
            )
        }
        is ArtistView.Discography -> Discography(runtime, artistId, view.albums)
        is ArtistView.Appearances -> Appearances(runtime, artistId, view)
    }
}

@Composable
private fun Discography(runtime: KoanRuntime, artistId: Long, albums: List<Album>) {
    val record by runtime.library.detailArtist.collectAsStateWithLifecycle()
    val favouriteArtists by runtime.library.favouriteArtistIds.collectAsStateWithLifecycle()
    val actions = rememberRecordActions(runtime)
    val navigator = runtime.navigator
    var pick by remember { mutableStateOf(Pick<Long>()) }
    BackHandler(enabled = pick.active) { pick = pick.end() }
    val artist = record?.artist
    val name = artist?.name ?: "Artist"
    val photo = hasPhoto(record)
    val albumIds = remember(albums) { albums.map { it.id } }
    val chosen = pick.picked
    val statusTop = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()

    BoxWithConstraints(Modifier.fillMaxSize().background(KoanTheme.colors.bg)) {
        val columns = gridColumns(maxWidth)
        val tile = tileWidth(maxWidth, columns)
        LazyVerticalGrid(
            columns = GridCells.Fixed(columns),
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(
                start = KoanSpace.page, end = KoanSpace.page,
                top = statusTop + KoanSpace.l, bottom = if (pick.active) 160.dp else KoanSpace.xxl,
            ),
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.l),
            verticalArrangement = Arrangement.spacedBy(KoanSpace.xl),
        ) {
            item(key = "header", span = { GridItemSpan(maxLineSpan) }, contentType = "header") {
                RecordHeader(
                    title = name,
                    columns = columns,
                    inset = 0.dp,
                    cover = if (photo) { size -> Sleeve(ArtworkSource.Artist(artistId), size, description = "Photograph of $name") } else null,
                    subtitle = { KoanText(countOf(albums.size, "album"), KoanRole.Control, KoanTone.Muted) },
                    actions = {
                        ArtistActions(runtime, artistId, name, artistId in favouriteArtists, actions, pick, albumIds) { pick = it }
                    },
                )
            }
            item(key = "bio", span = { GridItemSpan(maxLineSpan) }, contentType = "bio") {
                Biography(bioState(record))
            }
            items(albums, key = { it.id }, contentType = { "album" }) { album ->
                AlbumTile(
                    album = album, size = tile, selecting = pick.active, picked = album.id in pick,
                    onTap = { if (pick.active) pick = pick.toggle(album.id) else navigator.openAlbum(album.id) },
                    onLongPress = { pick = if (pick.active) pick.toggle(album.id) else pick.begin(album.id) },
                )
            }
        }
        if (pick.active) {
            val some = chosen.isNotEmpty()
            SelectionBar(
                pick.count,
                listOf(
                    BarAction("Play", { actions.playAlbums(chosen); pick = pick.end() }, some),
                    BarAction("Shuffle", { actions.playAlbums(chosen, shuffled = true); pick = pick.end() }, some),
                    BarAction("Play next", { actions.playNextAlbums(chosen); pick = pick.end() }, some),
                    BarAction("Add to queue", { actions.enqueueAlbums(chosen); pick = pick.end() }, some),
                    BarAction("Download", { actions.downloadAlbums(chosen); pick = pick.end() }, some),
                    BarAction("All", { pick = pick.selectAll(albumIds) }),
                    BarAction("Done", { pick = pick.end() }),
                ),
                Modifier.align(Alignment.BottomCenter),
            )
        }
    }
}

/** What the header offers on the artist as a whole. */
@Composable
private fun ArtistActions(
    runtime: KoanRuntime,
    artistId: Long,
    name: String,
    favourite: Boolean,
    actions: RecordActions,
    pick: Pick<Long>,
    albumIds: List<Long>,
    onPick: (Pick<Long>) -> Unit,
) {
    KoanButton("Play", { actions.playArtist(artistId, name, shuffled = false) }, kind = KoanButtonKind.Prominent)
    KoanButton("Shuffle", { actions.playArtist(artistId, name, shuffled = true) }, kind = KoanButtonKind.Bordered)
    KoanButton("Play next", { actions.playNextArtist(artistId) }, kind = KoanButtonKind.Compact)
    KoanButton("Add to queue", { actions.enqueueArtist(artistId) }, kind = KoanButtonKind.Compact)
    KoanButton(
        "Select", { onPick(if (pick.active) pick.end() else pick.begin()) },
        kind = KoanButtonKind.Compact, enabled = albumIds.isNotEmpty(),
    )
    KoanIconButton(
        icon = if (favourite) "heart-filled" else "heart",
        description = if (favourite) "Unfavourite $name" else "Favourite $name",
        onClick = { runtime.library.toggleFavouriteArtist(artistId) },
        tone = if (favourite) KoanTone.Accent else KoanTone.Muted,
    )
}

/** One record in the grid: its cover at the column's width, its title and year. */
@Composable
private fun AlbumTile(
    album: Album,
    size: Dp,
    selecting: Boolean,
    picked: Boolean,
    onTap: () -> Unit,
    onLongPress: () -> Unit,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .combinedClickable(role = Role.Button, onClick = onTap, onLongClick = onLongPress)
            .semantics(mergeDescendants = true) {
                if (selecting) stateDescription = if (picked) "selected" else "not selected"
            },
        verticalArrangement = Arrangement.spacedBy(KoanSpace.xs),
    ) {
        Box {
            Sleeve(ArtworkSource.Album(album.id), size, description = null)
            if (selecting) {
                Box(Modifier.padding(KoanSpace.s).background(KoanTheme.colors.bg)) {
                    KoanIcon(if (picked) "selecting" else "select", tone = if (picked) KoanTone.Accent else KoanTone.Muted)
                }
            }
        }
        KoanText(displayTitle(album.title), KoanRole.Control, KoanTone.Strong, data = true, maxLines = 2)
        album.year?.let { KoanText(it.toString(), KoanRole.Fine, KoanTone.Muted) }
    }
}

/**
 * The opening of the artist's Wikipedia article, credited as its licence asks;
 * a spinner-less line while the lookup is under way; nothing if there is none.
 */
@Composable
private fun Biography(state: BioState) {
    when (state) {
        BioState.Hidden -> Unit
        BioState.Looking -> KoanText("Looking up biography…", KoanRole.Control, KoanTone.Muted, modifier = Modifier.padding(top = KoanSpace.s))
        is BioState.Text -> {
            var expanded by rememberSaveable { mutableStateOf(false) }
            val uri = LocalUriHandler.current
            Column(Modifier.padding(top = KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
                KoanText(
                    state.paragraphs.joinToString("\n\n"), KoanRole.Meta, KoanTone.Muted, data = true,
                    maxLines = if (expanded) Int.MAX_VALUE else 4,
                )
                if (!expanded) {
                    KoanButton("More", { expanded = true }, kind = KoanButtonKind.Link)
                } else {
                    state.source?.let { source ->
                        KoanButton("From Wikipedia", { runCatching { uri.openUri(source) } }, kind = KoanButtonKind.Link)
                    }
                    state.imageCredit?.let { KoanText("Photo: $it", KoanRole.Fine, KoanTone.Muted, data = true, maxLines = 3) }
                }
            }
        }
    }
}

/** An artist who owns no records: the tracks they are credited on, each with its album. */
@Composable
private fun Appearances(runtime: KoanRuntime, artistId: Long, view: ArtistView.Appearances) {
    val record by runtime.library.detailArtist.collectAsStateWithLifecycle()
    val favouriteArtists by runtime.library.favouriteArtistIds.collectAsStateWithLifecycle()
    val favourites by runtime.library.favouriteTrackIds.collectAsStateWithLifecycle()
    val queued by runtime.mirror.queueIndex.collectAsStateWithLifecycle()
    val mirror = runtime.mirror
    val current by remember(mirror) { mirror.playback.map { it.entry?.trackId }.distinctUntilChanged() }
        .collectAsStateWithLifecycle(mirror.playback.value.entry?.trackId)
    val actions = rememberRecordActions(runtime)
    var pick by remember { mutableStateOf(Pick<Long>()) }
    BackHandler(enabled = pick.active) { pick = pick.end() }
    val tracks = view.tracks
    val ids = remember(tracks) { tracks.map { it.id } }
    val name = record?.artist?.name ?: "Artist"
    val photo = hasPhoto(record)
    val numberWidth = remember(tracks.size) { numberColumn(tracks.size) }
    val statusTop = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val chosen = remember(pick.picked, ids) { ids.toSet().let { held -> pick.picked.filter { it in held } } }

    BoxWithConstraints(Modifier.fillMaxSize().background(KoanTheme.colors.bg)) {
        val columns = gridColumns(maxWidth)
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = PaddingValues(top = statusTop + KoanSpace.l, bottom = if (pick.active) 160.dp else KoanSpace.xxl),
        ) {
            item(key = "header", contentType = "header") {
                RecordHeader(
                    title = name,
                    columns = columns,
                    cover = if (photo) { size -> Sleeve(ArtworkSource.Artist(artistId), size, description = "Photograph of $name") } else null,
                    subtitle = {
                        KoanText("Appears on ${countOf(view.albumCount, "album")}", KoanRole.Control, KoanTone.Muted)
                    },
                    actions = {
                        KoanButton("Play", { actions.play(ids) }, kind = KoanButtonKind.Prominent)
                        KoanButton("Shuffle", { actions.playArtist(artistId, name, shuffled = true) }, kind = KoanButtonKind.Bordered)
                        KoanButton("Play next", { actions.playNext(ids) }, kind = KoanButtonKind.Compact)
                        KoanButton("Add to queue", { actions.enqueue(ids) }, kind = KoanButtonKind.Compact)
                        KoanButton("Select", { pick = if (pick.active) pick.end() else pick.begin() }, kind = KoanButtonKind.Compact)
                        KoanIconButton(
                            icon = if (artistId in favouriteArtists) "heart-filled" else "heart",
                            description = if (artistId in favouriteArtists) "Unfavourite $name" else "Favourite $name",
                            onClick = { runtime.library.toggleFavouriteArtist(artistId) },
                            tone = if (artistId in favouriteArtists) KoanTone.Accent else KoanTone.Muted,
                        )
                    },
                )
            }
            item(key = "bio", contentType = "bio") {
                Box(Modifier.padding(horizontal = KoanSpace.page)) { Biography(bioState(record)) }
            }
            itemsIndexed(tracks, key = { index, track -> "track:${track.id}:$index" }, contentType = { _, _ -> "track" }) { index, track ->
                TrackRowView(
                    track = track,
                    position = index + 1,
                    numberWidth = numberWidth,
                    showsAlbum = true,
                    state = RowState(
                        current = current == track.id,
                        favourite = track.id in favourites,
                        availability = availability(track, queued.byTrack[track.id]),
                        selecting = pick.active,
                        picked = track.id in pick,
                        highlighted = false,
                    ),
                    onTap = { if (pick.active) pick = pick.toggle(track.id) else actions.play(ids, index) },
                    onLongPress = { pick = if (pick.active) pick.toggle(track.id) else pick.begin(track.id) },
                    onHeart = { actions.toggleFavouriteTrack(track.id) },
                )
            }
        }
        if (pick.active) {
            val some = chosen.isNotEmpty()
            SelectionBar(
                pick.count,
                listOf(
                    BarAction("Play", { actions.play(chosen); pick = pick.end() }, some),
                    BarAction("Play next", { actions.playNext(chosen); pick = pick.end() }, some),
                    BarAction("Add to queue", { actions.enqueue(chosen); pick = pick.end() }, some),
                    BarAction("Download", { actions.download(chosen); pick = pick.end() }, some),
                    BarAction("Remove downloads", { actions.removeDownloads(chosen); pick = pick.end() }, some),
                    BarAction("All", { pick = pick.selectAll(ids) }),
                    BarAction("Done", { pick = pick.end() }),
                ),
                Modifier.align(Alignment.BottomCenter),
            )
        }
    }
}

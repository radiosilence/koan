package cc.blit.koan.ui.library

import android.content.Context
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filter

/** The track playing, and whether it is moving, for rows that mark it. */
@Composable
fun nowPlayingTrack(): Pair<Long?, Boolean> {
    val now by koanApp().mirror.nowPlaying.collectAsStateWithLifecycle()
    return now?.entry?.trackId to (now?.state == PlayState.PLAYING)
}

private fun Context.libraryPrefs() = getSharedPreferences("library", Context.MODE_PRIVATE)

/** The narrowest a record tile goes, as the Apple apps' grids. */
private val TileMinimum = 150.dp

/**
 * Every record, or a shelf's, as a grid or a list. The grid is lazy and keyed
 * by album, so ten thousand records scroll as well as ten, and the place it
 * was left at survives a visit to a record. The select button picks several
 * to play or queue together.
 */
@Composable
fun AlbumsPage(shelf: Shelf?) {
    val app = koanApp()
    val context = LocalContext.current
    val model = viewModel { AlbumsModel(app.mirror, shelf, context.libraryPrefs()) }
    val albums by model.albums.collectAsStateWithLifecycle()
    val query by model.query.collectAsStateWithLifecycle()
    val layout by model.layout.collectAsStateWithLifecycle()
    val choices by model.choices.collectAsStateWithLifecycle()
    val selection = model.selection
    SelectMode(selection)
    val narrowed = query.filter.activeCount > 0 || query.search.isNotBlank()
    val grid = rememberLazyGridState()
    val list = rememberLazyListState()
    Column {
        PageBar(
            title = "Albums",
            subtitle = albums?.takeIf { narrowed }?.let { count(it.size.toLong(), "album") },
        ) {
            val grid = layout == AlbumLayout.Grid
            KoanButton(
                if (grid) "Show as list" else "Show as grid",
                { model.layout(if (grid) AlbumLayout.List else AlbumLayout.Grid) },
                kind = KoanButtonKind.Icon,
                icon = if (grid) KoanIcons.queueSection else KoanIcons.album,
            )
            // Its own button, since it is pressed again and again.
            if (query.sort == AlbumSort.RANDOM) KoanButton("Shuffle", { model.sort(AlbumSort.RANDOM) }, kind = KoanButtonKind.Icon, icon = KoanIcons.reshuffle)
            SortButton(query.sort, query.offered(), { it.label }, model::sort)
            FilterButton(query.filter, choices, model::filter)
            if (!albums.isNullOrEmpty()) SelectButton(selection)
        }
        FilterField(query.search, "Filter albums", model::search)
        Box(Modifier.weight(1f)) {
            val shown = albums ?: return@Box
            when {
                shown.isEmpty() -> KoanUnavailable(
                    if (narrowed) "Nothing matches" else "No albums yet",
                    KoanIcons.album,
                    if (narrowed) "Try a different filter." else "Sign in to a server in Settings to see its library.",
                )
                layout == AlbumLayout.Grid -> AlbumGrid(shown, grid) { AlbumTile(it, selection = selection) }
                else -> LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                    items(shown, key = { it.id }, contentType = { "album" }) { AlbumRow(it, selection = selection) }
                }
            }
        }
        if (selection.active) SelectionBar(selection)
    }
}

/**
 * Records as a lazy grid, keyed by album: only the tiles on screen are
 * composed, and the place it is scrolled to follows the record at the top
 * when the listing changes around it.
 */
@Composable
fun AlbumGrid(albums: List<Album>, state: LazyGridState, modifier: Modifier = Modifier, tile: @Composable (Album) -> Unit) {
    val gutter = LocalKoanTokens.current.gutter
    LazyVerticalGrid(
        columns = GridCells.Adaptive(TileMinimum),
        state = state,
        contentPadding = PaddingValues(horizontal = gutter - 4.dp, vertical = 8.dp),
        modifier = modifier.fillMaxSize(),
    ) {
        items(albums, key = { it.id }, contentType = { "album" }) { tile(it) }
    }
}

@Composable
fun ArtistsPage(shelf: Shelf?) {
    val app = koanApp()
    val model = viewModel { ArtistsModel(app.mirror, shelf) }
    val artists by model.artists.collectAsStateWithLifecycle()
    val query by model.query.collectAsStateWithLifecycle()
    val choices by model.choices.collectAsStateWithLifecycle()
    val selection = model.selection
    SelectMode(selection)
    val narrowed = query.filter.activeCount > 0 || query.search.isNotBlank()
    val list = rememberLazyListState()
    Column {
        PageBar(
            title = "Artists",
            subtitle = artists?.takeIf { narrowed }?.let { count(it.size.toLong(), "artist") },
        ) {
            FilterButton(query.filter, choices, model::filter)
            if (!artists.isNullOrEmpty()) SelectButton(selection)
        }
        FilterField(query.search, "Filter artists", model::search)
        Box(Modifier.weight(1f)) {
            val shown = artists ?: return@Box
            if (shown.isEmpty()) {
                KoanUnavailable(
                    if (narrowed) "Nothing matches" else "No artists yet",
                    KoanIcons.artist,
                    if (narrowed) "Try a different filter." else null,
                )
            } else {
                LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                    items(shown, key = { it.id }, contentType = { "artist" }) { ArtistRow(it, selection = selection) }
                }
            }
        }
        if (selection.active) SelectionBar(selection)
    }
}

/** A shelf's tracks, a page at a time, each playing the list from itself. */
@Composable
fun TracksPage(shelf: Shelf) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val model = viewModel { TracksModel(app.mirror, shelf) }
    val page by model.page.collectAsStateWithLifecycle()
    val query by model.query.collectAsStateWithLifecycle()
    val choices by model.choices.collectAsStateWithLifecycle()
    val selection = model.selection
    SelectMode(selection)
    val (current, playing) = nowPlayingTrack()
    val list = rememberLazyListState()
    LaunchedEffect(list) {
        snapshotFlow { list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0 }
            .distinctUntilChanged()
            .filter { it >= (model.page.value?.tracks?.size ?: 0) - 100 }
            .collect { model.more() }
    }
    Column {
        PageBar(
            title = "${shelf.title} · Tracks",
            subtitle = page?.let { count(it.total, "track") },
            appText = shelf !is Shelf.Search,
        ) {
            SortButton(query.sort, query.offered(), { it.label }, model::sort)
            FilterButton(query.filter, choices, model::filter)
            if (!page?.tracks.isNullOrEmpty()) SelectButton(selection)
        }
        FilterField(query.search, "Filter tracks", model::search)
        Box(Modifier.weight(1f)) {
            val shown = page ?: return@Box
            if (shown.tracks.isEmpty()) {
                // Narrowed beyond what the shelf itself is.
                val narrowed = shelf.trackQuery().let { query.filter != it.filter || query.search != it.search }
                return@Box KoanUnavailable(if (narrowed) "Nothing matches" else "No tracks", KoanIcons.track, if (narrowed) "Try a different filter." else null)
            }
            val ids = remember(shown) { shown.tracks.map { it.id } }
            LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                itemsIndexed(shown.tracks, key = { _, t -> t.id }, contentType = { _, _ -> "track" }) { index, track ->
                    TrackRow(
                        track,
                        lead = "${index + 1}",
                        current = track.id == current,
                        playing = playing,
                        onPlay = { app.playFrom(ids, index, nav) },
                        selection = selection,
                        mixed = true,
                    )
                }
            }
        }
        if (selection.active) SelectionBar(selection)
    }
}

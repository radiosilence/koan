package cc.blit.koan.ui.library

import android.content.Context
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.R
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
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

/**
 * Every record, or a shelf's, as a grid or a list. The grid is lazy and keyed
 * by album, so ten thousand records scroll as well as ten, and the place it
 * was left at survives a visit to a record.
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
    val narrowed = query.filter.activeCount > 0 || query.search.isNotBlank()
    val gutter = LocalKoanTokens.current.gutter
    val grid = rememberLazyGridState()
    val list = rememberLazyListState()
    Column {
        PageBar(
            title = "Albums",
            subtitle = albums?.takeIf { narrowed }?.let { count(it.size.toLong(), "album") },
        ) {
            IconButton(onClick = { model.layout(if (layout == AlbumLayout.Grid) AlbumLayout.List else AlbumLayout.Grid) }) {
                Icon(
                    painterResource(if (layout == AlbumLayout.Grid) R.drawable.ic_list else R.drawable.ic_grid),
                    if (layout == AlbumLayout.Grid) "Show as list" else "Show as grid",
                )
            }
            SortButton(query.sort, query.offered(), { it.label }, model::sort)
            FilterButton(query.filter, choices, model::filter)
        }
        FilterField(query.search, "Filter albums", model::search)
        val shown = albums ?: return@Column
        when {
            shown.isEmpty() -> EmptyState(
                if (narrowed) "Nothing matches" else "No albums yet",
                if (narrowed) "Try a different filter." else "Sign in to a server in Settings to see its library.",
            )
            layout == AlbumLayout.Grid -> LazyVerticalGrid(
                columns = GridCells.Adaptive(150.dp),
                state = grid,
                contentPadding = PaddingValues(horizontal = gutter - 4.dp, vertical = 8.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                items(shown, key = { it.id }, contentType = { "album" }) { AlbumTile(it) }
            }
            else -> LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                items(shown, key = { it.id }, contentType = { "album" }) { AlbumRow(it) }
            }
        }
    }
}

@Composable
fun ArtistsPage(shelf: Shelf?) {
    val app = koanApp()
    val model = viewModel { ArtistsModel(app.mirror, shelf) }
    val artists by model.artists.collectAsStateWithLifecycle()
    val query by model.query.collectAsStateWithLifecycle()
    val choices by model.choices.collectAsStateWithLifecycle()
    val narrowed = query.filter.activeCount > 0 || query.search.isNotBlank()
    val list = rememberLazyListState()
    Column {
        PageBar(
            title = "Artists",
            subtitle = artists?.takeIf { narrowed }?.let { count(it.size.toLong(), "artist") },
        ) {
            FilterButton(query.filter, choices, model::filter)
        }
        FilterField(query.search, "Filter artists", model::search)
        val shown = artists ?: return@Column
        if (shown.isEmpty()) {
            EmptyState(if (narrowed) "Nothing matches" else "No artists yet", if (narrowed) "Try a different filter." else null)
        } else {
            LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                items(shown, key = { it.id }, contentType = { "artist" }) { ArtistRow(it) }
            }
        }
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
    val (current, playing) = nowPlayingTrack()
    val list = rememberLazyListState()
    LaunchedEffect(list) {
        snapshotFlow { list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0 }
            .distinctUntilChanged()
            .filter { it >= (model.page.value?.tracks?.size ?: 0) - 100 }
            .collect { model.more() }
    }
    Column {
        PageBar(title = "${shelf.title} · Tracks", subtitle = page?.let { count(it.total, "track") }) {
            SortButton(query.sort, query.offered(), { it.label }, model::sort)
        }
        val shown = page ?: return@Column
        if (shown.tracks.isEmpty()) return@Column EmptyState("No tracks")
        val ids = remember(shown) { shown.tracks.map { it.id } }
        LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
            itemsIndexed(shown.tracks, key = { _, t -> t.id }, contentType = { _, _ -> "track" }) { index, track ->
                TrackRow(
                    track,
                    lead = "${index + 1}",
                    current = track.id == current,
                    playing = playing,
                    onPlay = { app.playFrom(ids, index, nav) },
                    mixed = true,
                )
            }
        }
    }
}

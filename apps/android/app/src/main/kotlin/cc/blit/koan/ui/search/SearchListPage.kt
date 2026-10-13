package cc.blit.koan.ui.search

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material3.Text
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.AlbumSort
import cc.blit.koan.ffi.Artist
import cc.blit.koan.ffi.Track
import cc.blit.koan.ffi.TrackBrowseSort
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.NoFilter
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.LocalKoanTokens
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.stateIn

/** A listing's contents so far, and how many the query finds in all. */
data class Listing(
    val artists: List<Artist> = emptyList(),
    val albums: List<Album> = emptyList(),
    val tracks: List<Track> = emptyList(),
    val total: Long = 0,
)

/**
 * Everything of one kind a query finds, in the search shelf's order: the
 * library's own listings (`artists`, `albums`, `trackListing`) narrowed by the
 * search, as the library's browsers are. Tracks come a page at a time, since a
 * library has tens of thousands.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class SearchListModel(mirror: EngineMirror, list: SearchList, query: String) : ViewModel() {
    /** How many tracks to have read; raised as the list scrolls towards its end. */
    private val wanted = MutableStateFlow(PAGE)

    val listing: StateFlow<Listing?> = combine(mirror.library, wanted) { _, wanted -> wanted }
        .mapLatest { wanted ->
            runCatching {
                val engine = mirror.engine()
                when (list) {
                    SearchList.Artists -> engine.artists(query, NoFilter).let { Listing(artists = it, total = it.size.toLong()) }
                    SearchList.Albums -> engine.albums(null, AlbumSort.BEST_MATCH, 0, query, NoFilter)
                        .let { Listing(albums = it, total = it.size.toLong()) }
                    SearchList.Tracks -> engine.trackListing(TrackBrowseSort.BEST_MATCH, query, NoFilter, wanted.toUInt(), 0u)
                        .let { Listing(tracks = it.tracks, total = it.total.toLong()) }
                }
            }.getOrNull()
        }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    val selection = Selection()

    fun more(seen: Int) {
        val have = listing.value ?: return
        if (have.tracks.size < have.total && seen >= wanted.value - PAGE / 2) wanted.value += PAGE
    }

    companion object {
        const val PAGE = 200
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SearchListPage(list: SearchList, query: String) {
    val app = koanApp()
    val model = viewModel { SearchListModel(app.mirror, list, query) }
    val listing by model.listing.collectAsStateWithLifecycle()
    val selection = model.selection
    val nav = LocalNavigator.current
    val tokens = LocalKoanTokens.current
    val grid = rememberLazyGridState()
    var replacing by remember { mutableStateOf<Album?>(null) }

    DisposableEffect(Unit) { onDispose { selection.end() } }
    // Composed only while picking, so it registers after the shell's own
    // handler for a tab's root and is asked first.
    if (selection.active) BackHandler { selection.end() }
    LaunchedEffect(grid) {
        snapshotFlow { grid.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0 }
            .distinctUntilChanged()
            .filter { list == SearchList.Tracks }
            .collect(model::more)
    }

    Column(Modifier.fillMaxSize()) {
        Box(Modifier.weight(1f)) {
            val shown = listing ?: return@Box
            LazyVerticalGrid(
                state = grid,
                columns = GridCells.Adaptive(TileMinimum),
                contentPadding = PaddingValues(tokens.gutter),
                horizontalArrangement = Arrangement.spacedBy(tokens.gutter),
                verticalArrangement = Arrangement.spacedBy(tokens.rowGap),
            ) {
                item(key = "title", span = { GridItemSpan(maxLineSpan) }) {
                    Text("${list.name} for “$query”", style = MaterialTheme.typography.titleLarge)
                }
                when (list) {
                    SearchList.Artists -> item(key = "artists", span = { GridItemSpan(maxLineSpan) }) {
                        FlowRow(
                            horizontalArrangement = Arrangement.spacedBy(tokens.rowGap),
                            verticalArrangement = Arrangement.spacedBy(tokens.rowGap),
                        ) {
                            shown.artists.forEach { ArtistPill(Playable.Artist(it.id, it.name), selection) }
                        }
                    }
                    SearchList.Albums -> items(shown.albums, key = { it.id }) { album ->
                        AlbumTile(album, selection, onSleeve = {
                            if (app.mirror.queue.value.items.isEmpty()) playRecord(app, nav, album) else replacing = album
                        })
                    }
                    SearchList.Tracks -> items(shown.tracks, key = { it.id }, span = { GridItemSpan(maxLineSpan) }) {
                        TrackRow(it, selection)
                    }
                }
            }
        }
        if (selection.active) SelectionBar(selection)
    }

    replacing?.let { ReplaceQueueDialog(it, onDismiss = { replacing = null }) }
}

package cc.blit.koan.ui.pages

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.background
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import cc.blit.koan.state.label
import cc.blit.koan.state.offeredAlbumSorts
import cc.blit.koan.state.activeCount
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import uniffi.koan_ffi.Album
import uniffi.koan_ffi.AlbumSort

private val TileMin = 140.dp
private val TileGap = 14.dp
private val GridPad = KoanSpace.l

/**
 * The album grid (Section.Albums), as `AlbumBrowser` draws it: sleeves in a
 * lazy grid, a tile opening its record, the heart showing a favourite.
 *
 * The listing is `library.visibleAlbums`, whole, as the database sorted and
 * narrowed it; the sort row is [offeredAlbumSorts] over `setAlbumSort`.
 * Tiles are keyed by album id and typed, so a reshuffle moves them rather
 * than rebuilding them, and one tile size is worked out for the whole grid
 * from the page width so no tile measures itself to size its sleeve.
 */
@Composable
fun AlbumsPage(runtime: KoanRuntime) {
    val library = runtime.library
    val tab = rememberHomeTab(runtime)
    val albums by library.visibleAlbums.collectAsStateWithLifecycle()
    val favourites by library.favouriteAlbumIds.collectAsStateWithLifecycle()
    val filter by library.filter.collectAsStateWithLifecycle()
    val browse by library.browseFilter.collectAsStateWithLifecycle()
    val sort by library.albumSort.collectAsStateWithLifecycle()
    val signedIn by library.signedIn.collectAsStateWithLifecycle()
    val narrowed = filter.isNotEmpty() || browse.activeCount > 0
    val grid = rememberRestoredGridState(runtime, tab, Section.Albums)
    val navigator = runtime.navigator
    val open = remember(navigator) { { id: Long -> navigator.openAlbum(id) } }
    val toggle = remember(library) { { id: Long -> library.toggleFavouriteAlbum(id) } }

    Column(Modifier.fillMaxSize()) {
        BrowseHeader(runtime, Section.Albums, if (narrowed) countLabel(albums.size.toLong(), "album") else null) {
            val offered = offeredAlbumSorts(browse.recent, browse.downloaded, filter.isNotEmpty())
            Row(verticalAlignment = Alignment.CenterVertically) {
                ChoiceStrip(
                    options = offered.map { it to it.label },
                    selected = sort,
                    onSelect = { it?.let(library::setAlbumSort) },
                    modifier = Modifier.weight(1f),
                )
                if (sort == AlbumSort.RANDOM) {
                    KoanButton("shuffle", library::reshuffleAlbums, kind = KoanButtonKind.Text, icon = "dice")
                }
            }
        }
        if (albums.isEmpty()) {
            EmptyNote(
                "album",
                if (narrowed) "Nothing matches" else "No albums yet",
                if (narrowed) "Try a different filter." else library.emptyLibraryDetail(signedIn),
            )
        } else {
            BoxWithConstraints(Modifier.fillMaxSize()) {
                val density = LocalDensity.current
                val tile = remember(maxWidth, density) {
                    with(density) {
                        val avail = (maxWidth - GridPad * 2).roundToPx()
                        tileWidths(avail, TileGap.roundToPx(), TileMin.roundToPx()).first().toDp()
                    }
                }
                LazyVerticalGrid(
                    columns = TileCells(TileMin),
                    state = grid,
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(GridPad),
                    horizontalArrangement = Arrangement.spacedBy(TileGap),
                    verticalArrangement = Arrangement.spacedBy(KoanSpace.l),
                ) {
                    items(albums, key = { it.id }, contentType = { "album" }) { album ->
                        AlbumTile(album, tile, album.id in favourites, open, toggle)
                    }
                }
            }
        }
    }
}

/**
 * One record: its sleeve at the grid's tile size with the codec in a corner
 * and a heart if it is a favourite, then title and artist. A tap opens it; a
 * long press toggles the favourite.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun AlbumTile(
    album: Album,
    size: Dp,
    favourite: Boolean,
    onOpen: (Long) -> Unit,
    onToggleFavourite: (Long) -> Unit,
    modifier: Modifier = Modifier,
    showArtist: Boolean = true,
) {
    val colors = KoanTheme.colors
    Column(
        modifier.combinedClickable(
            role = Role.Button,
            onLongClickLabel = if (favourite) "Remove from favourites" else "Add to favourites",
            onLongClick = { onToggleFavourite(album.id) },
            onClick = { onOpen(album.id) },
        ),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.xs),
    ) {
        Box {
            Sleeve(ArtworkSource.Album(album.id), size)
            album.codec?.let {
                KoanText(
                    it.uppercase(), KoanRole.Fine, KoanTone.Ink, data = true,
                    modifier = Modifier.align(Alignment.TopEnd).background(colors.bg).padding(horizontal = KoanSpace.xs),
                )
            }
            if (favourite) {
                KoanIcon(
                    "heart-filled", tone = KoanTone.Accent, textSizeDp = 14f, description = "Favourite",
                    modifier = Modifier.align(Alignment.BottomStart).padding(KoanSpace.xs),
                )
            }
        }
        KoanText(album.title, KoanRole.Control, KoanTone.Strong, data = true, modifier = Modifier.width(size))
        val subtitle = listOfNotNull(if (showArtist) album.artistName else null, album.year?.toString()).joinToString(" · ")
        if (subtitle.isNotEmpty()) KoanText(subtitle, KoanRole.Fine, KoanTone.Muted, data = true, modifier = Modifier.width(size))
    }
}

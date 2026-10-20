package cc.blit.koan.ui.search

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridScope
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.clearText
import androidx.compose.foundation.text.input.rememberTextFieldState
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.KoanApp
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.ShelfKind
import cc.blit.koan.ffi.ShelfSummary
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.library.AlbumsRoute
import cc.blit.koan.ui.library.ArtistPill
import cc.blit.koan.ui.library.ArtistsRoute
import cc.blit.koan.ui.library.PageBar
import cc.blit.koan.ui.library.Playable
import cc.blit.koan.ui.library.SectionHead
import cc.blit.koan.ui.library.Selection
import cc.blit.koan.ui.library.SelectionBar
import cc.blit.koan.ui.library.Shelf
import cc.blit.koan.ui.library.TracksRoute
import cc.blit.koan.ui.library.play
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Navigator
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanEdge
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.koanRule

/**
 * Search, as its own tab: the field always showing at the top, and what it
 * finds as artists, records and tracks beneath, each heading opening the
 * library's browser on the search. A result goes where it lives in the
 * library; a record's sleeve plays it. The select button picks several to
 * play or queue together.
 */
@Composable
fun SearchPage() {
    val app = koanApp()
    val model = viewModel {
        SearchModel({ app.mirror.engine().shelfSummary(ShelfKind.Search(it)) }, app.mirror.library)
    }
    val query by model.query.collectAsStateWithLifecycle()
    val results by model.results.collectAsStateWithLifecycle()
    val selection = model.selection
    val nav = LocalNavigator.current

    // The field's own state, which the model follows; seeded from the model so
    // the query survives the page leaving and coming back.
    val field = rememberTextFieldState(query)
    LaunchedEffect(field) { snapshotFlow { field.text.toString() }.collect(model::setQuery) }

    // A pick belongs to the page it was made on.
    DisposableEffect(Unit) { onDispose { selection.end() } }
    // Composed only while picking, so it registers after the shell's own
    // handler for a tab's root and is asked first.
    if (selection.active) BackHandler { selection.end() }

    val shown = results?.takeIf { query.isNotBlank() }
    val searching = query.isNotBlank() && shown?.query != query.trim()

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().koanRule(KoanEdge.Bottom).padding(horizontal = KoanSpace.l, vertical = KoanSpace.xs),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            SearchField(field, Modifier.weight(1f))
            if (shown?.isEmpty == false || selection.active) {
                KoanButton({ if (selection.active) selection.end() else selection.begin() }, kind = KoanButtonKind.Icon) {
                    KoanIcon(if (selection.active) KoanIcons.selecting else KoanIcons.select, "Select")
                }
            }
        }
        // Under the field rather than over it, so the field stays put as the
        // first letter is typed.
        shown?.query?.let { PageBar("${KoanTheme.label("Results for")} “$it”", back = false, appText = false) }
        Box(Modifier.weight(1f)) {
            val shelf = shown?.shelf
            when {
                query.isBlank() -> KoanUnavailable("Search your library", KoanIcons.search, null)
                shown == null || (shown.isEmpty && searching) -> Unit
                shelf == null || shown.isEmpty ->
                    KoanUnavailable("Nothing found", KoanIcons.search, "No artists, albums or tracks match “${shown.query}”.")
                else -> Found(shown.query, shelf, selection)
            }
        }
        if (selection.active) SelectionBar(selection)
    }
}

/**
 * The query: no box in the theme, a glyph before it and a clear button after.
 * The keyboard's search key only puts the keyboard away, since results arrive
 * as the query is typed.
 */
@Composable
private fun SearchField(field: TextFieldState, modifier: Modifier) {
    val keyboard = LocalSoftwareKeyboardController.current
    val ink = KoanTheme.color(KoanTone.Ink)
    Row(modifier.heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
        KoanIcon(KoanIcons.search, null, color = KoanTheme.color(KoanTone.Muted))
        BasicTextField(
            state = field,
            modifier = Modifier.weight(1f).padding(horizontal = KoanSpace.m),
            textStyle = KoanType.Body.koan.copy(color = ink),
            cursorBrush = SolidColor(KoanTheme.color(KoanTone.Accent)),
            lineLimits = TextFieldLineLimits.SingleLine,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            onKeyboardAction = { keyboard?.hide() },
            decorator = { inner ->
                Box(contentAlignment = Alignment.CenterStart) {
                    if (field.text.isEmpty()) KoanText("Artists, albums, tracks", tone = KoanTone.Muted, appText = true, maxLines = 1)
                    inner()
                }
            },
        )
        if (field.text.isNotEmpty()) {
            KoanButton({ field.clearText() }, kind = KoanButtonKind.Icon) {
                KoanIcon(KoanIcons.clearField, "Clear")
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Found(query: String, shelf: ShelfSummary, selection: Selection<Playable>) {
    val nav = LocalNavigator.current
    val search = Shelf.Search(query)
    LazyVerticalGrid(
        columns = GridCells.Adaptive(TileMinimum),
        contentPadding = PaddingValues(horizontal = KoanSpace.page, vertical = KoanSpace.l),
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.l),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        if (shelf.artists.isNotEmpty()) {
            heading("Artists", shelf.artistTotal) { nav.push(ArtistsRoute(search)) }
            item(key = "artists", span = { GridItemSpan(maxLineSpan) }) {
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
                    verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
                ) {
                    shelf.artists.forEach { ArtistPill(it.id, it.name, selection) }
                }
            }
        }
        if (shelf.albums.isNotEmpty()) {
            heading("Albums", shelf.albumTotal) { nav.push(AlbumsRoute(search)) }
            items(shelf.albums, key = { "album:${it.id}" }) { album ->
                AlbumTile(album, selection)
            }
        }
        if (shelf.tracks.isNotEmpty()) {
            heading("Tracks", shelf.trackTotal) { nav.push(TracksRoute(search)) }
            items(shelf.tracks, key = { "track:${it.id}" }, span = { GridItemSpan(maxLineSpan) }) { track ->
                TrackRow(track, selection)
            }
        }
    }
}

/** A section's heading, the library's own, opening the browser on the whole set. */
private fun LazyGridScope.heading(title: String, total: ULong, open: () -> Unit) {
    item(key = "heading:$title", span = { GridItemSpan(maxLineSpan) }) {
        SectionHead(title, total.toLong(), open)
    }
}

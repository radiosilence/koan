package cc.blit.koan.ui.search

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridScope
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.clearText
import androidx.compose.foundation.text.input.rememberTextFieldState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconToggleButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
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
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.KoanApp
import cc.blit.koan.R
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.ShelfKind
import cc.blit.koan.ffi.ShelfSummary
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.Navigator
import cc.blit.koan.ui.theme.LocalKoanTokens

/**
 * Search, as its own tab: the field always showing at the top, and what it
 * finds as artists, records and tracks beneath, each section's heading opening
 * the whole listing. A result goes where it lives in the library; a record's
 * sleeve plays it. Long-press, or the select button, picks several to play or
 * queue together.
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
    val tokens = LocalKoanTokens.current
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

    var replacing by remember { mutableStateOf<Album?>(null) }
    val shown = results?.takeIf { query.isNotBlank() }
    val searching = query.isNotBlank() && shown?.query != query.trim()

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = tokens.gutter, vertical = tokens.rowGap),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            SearchField(field, Modifier.weight(1f))
            if (shown?.isEmpty == false || selection.active) {
                IconToggleButton(
                    checked = selection.active,
                    onCheckedChange = { if (it) selection.begin() else selection.end() },
                ) {
                    Icon(painterResource(R.drawable.ic_checklist), "Select")
                }
            }
        }
        Box(Modifier.weight(1f)) {
            val shelf = shown?.shelf
            when {
                query.isBlank() -> Empty("Search your library", null)
                shown == null || (shown.isEmpty && searching) -> Unit
                shelf == null || shown.isEmpty -> Empty("Nothing found", "No artists, albums or tracks match “${shown.query}”.")
                else -> Found(shown.query, shelf, selection, onSleeve = { album ->
                    if (app.mirror.queue.value.items.isEmpty()) playRecord(app, nav, album) else replacing = album
                })
            }
        }
        if (selection.active) SelectionBar(selection)
    }

    replacing?.let { album ->
        ReplaceQueueDialog(album, onDismiss = { replacing = null })
    }
}

@Composable
private fun SearchField(field: TextFieldState, modifier: Modifier) {
    val keyboard = LocalSoftwareKeyboardController.current
    TextField(
        state = field,
        modifier = modifier,
        placeholder = { Text("Artists, albums, tracks") },
        leadingIcon = { Icon(painterResource(R.drawable.ic_search), null) },
        trailingIcon = {
            if (field.text.isNotEmpty()) {
                IconButton(onClick = { field.clearText() }) {
                    Icon(painterResource(R.drawable.ic_clear), "Clear")
                }
            }
        },
        lineLimits = TextFieldLineLimits.SingleLine,
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
        // Results arrive as you type; the search key only puts the keyboard
        // away so they can be seen.
        onKeyboardAction = { keyboard?.hide() },
    )
}

@Composable
private fun Empty(title: String, detail: String?) {
    Column(
        Modifier.fillMaxSize().padding(LocalKoanTokens.current.gutter),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(painterResource(R.drawable.ic_search), null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(title, style = MaterialTheme.typography.titleMedium)
        detail?.let {
            Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Found(
    query: String,
    shelf: ShelfSummary,
    selection: Selection,
    onSleeve: (Album) -> Unit,
) {
    val nav = LocalNavigator.current
    val tokens = LocalKoanTokens.current
    LazyVerticalGrid(
        columns = GridCells.Adaptive(TileMinimum),
        contentPadding = PaddingValues(tokens.gutter),
        horizontalArrangement = Arrangement.spacedBy(tokens.gutter),
        verticalArrangement = Arrangement.spacedBy(tokens.rowGap),
    ) {
        if (shelf.artists.isNotEmpty()) {
            heading("Artists", shelf.artistTotal) { nav.push(SearchListRoute(SearchList.Artists, query)) }
            item(key = "artists", span = { GridItemSpan(maxLineSpan) }) {
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(tokens.rowGap),
                    verticalArrangement = Arrangement.spacedBy(tokens.rowGap),
                ) {
                    shelf.artists.forEach { artist -> ArtistPill(Playable.Artist(artist.id, artist.name), selection) }
                }
            }
        }
        if (shelf.albums.isNotEmpty()) {
            heading("Albums", shelf.albumTotal) { nav.push(SearchListRoute(SearchList.Albums, query)) }
            items(shelf.albums, key = { "album:${it.id}" }) { album ->
                AlbumTile(album, selection, onSleeve = { onSleeve(album) })
            }
        }
        if (shelf.tracks.isNotEmpty()) {
            heading("Tracks", shelf.trackTotal) { nav.push(SearchListRoute(SearchList.Tracks, query)) }
            items(shelf.tracks, key = { "track:${it.id}" }, span = { GridItemSpan(maxLineSpan) }) { track ->
                TrackRow(track, selection)
            }
        }
    }
}

private fun LazyGridScope.heading(title: String, total: ULong, open: () -> Unit) {
    item(key = "heading:$title", span = { GridItemSpan(maxLineSpan) }) {
        SectionHeading(title, total, open)
    }
}

/** Opens the record and plays it, in place of whatever was queued. */
internal fun playRecord(app: KoanApp, nav: Navigator, album: Album) {
    nav.push(AlbumRoute(album.id))
    app.mirror.launch { app.player.playTracks(trackIds(album.id, null)) }
}

/**
 * A sleeve tap plays the record, and a queue built up over an evening should
 * not go to a tap meant to open it: with anything queued, ask first.
 */
@Composable
internal fun ReplaceQueueDialog(album: Album, onDismiss: () -> Unit) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val queued by app.mirror.queue.collectAsStateWithLifecycle()
    val count = queued.items.size
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Replace the queue?") },
        text = { Text("Playing this record takes the $count ${if (count == 1) "track" else "tracks"} queued off.") },
        confirmButton = {
            TextButton(onClick = {
                onDismiss()
                playRecord(app, nav, album)
            }) { Text("Play") }
        },
        dismissButton = {
            Row {
                TextButton(onClick = onDismiss) { Text("Cancel") }
                TextButton(onClick = {
                    onDismiss()
                    app.mirror.launch { app.player.enqueue(trackIds(album.id, null)) }
                }) { Text("Add to Queue") }
            }
        },
    )
}

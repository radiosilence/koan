package cc.blit.koan.ui.pages

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.clearText
import androidx.compose.foundation.text.input.rememberTextFieldState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.state.ShelfList
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import uniffi.koan_ffi.ShelfKind

/**
 * Search (the Search tab's root): one field over artists, records and tracks,
 * results as you type. Selecting a result takes you to where it lives rather
 * than playing it; you play from the album or artist page. A long press picks
 * instead, across every kind, and the pick is played or queued from the bar.
 *
 * The results are the search shelf's: each section is the head of the listing
 * its heading opens through `library.browse(ShelfList, ShelfKind.Search)`, and
 * its number that listing's length. Typing, debounce and stale-answer dropping
 * are `SearchModel`'s; nothing here reaches the engine.
 */
@Composable
fun SearchPage(runtime: KoanRuntime) {
    val model = runtime.search
    val hasQuery by model.hasQuery.collectAsStateWithLifecycle()
    val results by model.results.collectAsStateWithLifecycle()
    val pick by model.selection.pick.collectAsStateWithLifecycle()
    val favourites by runtime.library.favouriteTrackIds.collectAsStateWithLifecycle()

    // A pick belongs to the page it was made in.
    DisposableEffect(model) { onDispose { model.selection.end() } }

    val actions = remember(runtime) {
        ShelfActions(
            openSection = { list: ShelfList ->
                // The query as it is now, not as it was when the page composed.
                val section = runtime.library.browse(list, ShelfKind.Search(model.query.value.trim()))
                runtime.navigator.show(section)
            },
            openArtist = runtime.navigator::openArtist,
            openAlbum = { runtime.navigator.openAlbum(it) },
            // A track points at its album: that is the place in the library it lives, and where you play it from.
            tapTrack = { track, _ -> track.albumId?.let { runtime.navigator.openAlbum(it, highlighting = track.id) } },
            toggleFavourite = { runtime.library.toggleFavouriteTrack(it.id) },
        )
    }

    Column(Modifier.fillMaxSize()) {
        SearchField(remember(model) { model.query.value }, model::setQuery, Modifier.padding(horizontal = KoanSpace.page, vertical = KoanSpace.m))
        Box(Modifier.weight(1f)) {
            when {
                !hasQuery -> ShelfEmpty("search", "search your library")
                results.isEmpty && !results.searching ->
                    ShelfEmpty("search", "nothing found", "No artists, albums or tracks match “${results.query}”.")
                else -> ShelfResults(
                    artists = results.artists,
                    albums = results.albums,
                    tracks = results.tracks,
                    totals = results.totals,
                    favouriteTracks = favourites,
                    pick = pick,
                    selection = model.selection,
                    actions = actions,
                )
            }
        }
        if (pick.active) {
            ShelfPickBar(
                pick,
                onPlay = { model.commit(model.selection, play = true) },
                onQueue = { model.commit(model.selection, play = false) },
                onAll = model.selection::selectAll,
                onDone = model.selection::end,
            )
        }
    }
}

/**
 * The search field: a square one-point outline, the text in `ink`, a clear
 * button once there is something to clear. The text is held by the field and
 * handed to the model as it changes; the model's [query] seeds it, so the
 * words survive leaving the tab. Private to search until the theme has one
 * field for every page.
 */
@Composable
private fun SearchField(query: String, onQuery: (String) -> Unit, modifier: Modifier = Modifier) {
    val state = rememberTextFieldState(query)
    val focus = LocalFocusManager.current
    val colors = KoanTheme.colors
    LaunchedEffect(state) {
        snapshotFlow { state.text.toString() }.collect { onQuery(it) }
    }
    Row(
        modifier
            .fillMaxWidth()
            .height(KoanShape.hit)
            .border(KoanShape.hairline, colors.rule, RectangleShape)
            .background(colors.surface, RectangleShape)
            .padding(start = KoanSpace.m),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        KoanIcon("search", tone = KoanTone.Muted)
        BasicTextField(
            state = state,
            modifier = Modifier.weight(1f),
            textStyle = KoanTheme.style(KoanRole.Body).copy(color = colors.ink),
            cursorBrush = SolidColor(colors.accent),
            lineLimits = TextFieldLineLimits.SingleLine,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            onKeyboardAction = { focus.clearFocus() },
            decorator = { inner ->
                Box(contentAlignment = Alignment.CenterStart) {
                    if (state.text.isEmpty()) KoanText("search your library", KoanRole.Body, KoanTone.Muted)
                    inner()
                }
            },
        )
        if (state.text.isNotEmpty()) {
            KoanIconButton("close", "Clear search", onClick = { state.clearText() }, tone = KoanTone.Muted)
        }
    }
}

package cc.blit.koan.ui.pages

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import cc.blit.koan.state.Playable
import cc.blit.koan.state.ShelfList
import cc.blit.koan.state.ShelfTotals
import uniffi.koan_ffi.ShelfKind

/**
 * A shelf as a page: what you favourited, what you played lately, what is on
 * this device. Artists, records and tracks that answer one question, the
 * first few of each as `koan_core::shelves` cuts them, each section's heading
 * opening the listing the preview is the head of (`library.browse(list,
 * shelf)`) with the count the heading gave.
 *
 * `visibleShelf` holds whichever shelf was read last, so the rows are drawn
 * only while the library is on [section]: a page left under another one in
 * the stack does not show that one's rows.
 */
@Composable
internal fun ShelfPage(
    runtime: KoanRuntime,
    section: Section,
    shelf: ShelfKind,
    title: String,
    emptyIcon: String,
    emptyTitle: String,
    emptyDetail: String,
) {
    val summary by runtime.library.visibleShelf.collectAsStateWithLifecycle()
    val onSection by runtime.library.section.collectAsStateWithLifecycle()
    val favourites by runtime.library.favouriteTrackIds.collectAsStateWithLifecycle()
    val model = runtime.search

    val shown = if (onSection == section) summary else null
    val latest = rememberUpdatedState(shown)
    val selection = remember(model) {
        model.newSelection {
            latest.value?.let { s ->
                s.artists.map { Playable.OfArtist(it.id, it.name) } +
                    s.albums.map(Playable::OfAlbum) +
                    s.tracks.map(Playable::OfTrack)
            }.orEmpty()
        }
    }
    val pick by selection.pick.collectAsStateWithLifecycle()
    DisposableEffect(selection) { onDispose { selection.end() } }

    val actions = remember(runtime, shelf) {
        ShelfActions(
            openSection = { list: ShelfList -> runtime.navigator.show(runtime.library.browse(list, shelf)) },
            openArtist = runtime.navigator::openArtist,
            openAlbum = { runtime.navigator.openAlbum(it) },
            // Plays from this track, keeping the rest of the list behind it.
            tapTrack = { _, index -> latest.value?.let { model.playTracks(it.tracks.map { t -> t.id }, index) } },
            toggleFavourite = { runtime.library.toggleFavouriteTrack(it.id) },
        )
    }

    val current = shown
    Column(Modifier.fillMaxSize()) {
        Box(Modifier.weight(1f)) {
            if (current == null) {
                // Not read yet; the navigator reads a page before it moves to it, so this is a frame at most.
                Box(Modifier.fillMaxSize())
            } else if (current.artists.isEmpty() && current.albums.isEmpty() && current.tracks.isEmpty()) {
                ShelfEmpty(emptyIcon, emptyTitle, emptyDetail)
            } else {
                ShelfResults(
                    artists = current.artists,
                    albums = current.albums,
                    tracks = current.tracks,
                    totals = ShelfTotals.of(current),
                    favouriteTracks = favourites,
                    pick = pick,
                    selection = selection,
                    actions = actions,
                    header = { ShelfHeader(title, ShelfTotals.of(current).text()) },
                )
            }
        }
        if (pick.active) {
            ShelfPickBar(
                pick,
                onPlay = { model.commit(selection, play = true) },
                onQueue = { model.commit(selection, play = false) },
                onAll = selection::selectAll,
                onDone = selection::end,
            )
        }
    }
}

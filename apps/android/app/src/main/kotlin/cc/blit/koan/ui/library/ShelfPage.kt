package cc.blit.koan.ui.library

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.ffi.ShelfSummary
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens

/**
 * A page of artists, records and tracks answering one question: what is
 * favourited, played lately, or on this device. The first few of each, as
 * `koan_core::shelves` cuts them; each heading gives how many there are and
 * opens that kind's browser filtered to the shelf, the listing the preview is
 * the head of.
 *
 * Sections rather than a type picker: they all answer one question, and a
 * mode to remember being in is a worse way to find a record.
 */
@Composable
fun ShelfPage(shelf: Shelf) {
    val app = koanApp()
    val model = viewModel { ShelfModel(app.mirror, shelf) }
    val summary by model.summary.collectAsStateWithLifecycle()
    val selection = model.selection
    SelectMode(selection)
    val empty = summary?.run { artists.isEmpty() && albums.isEmpty() && tracks.isEmpty() } ?: true
    Column {
        PageBar(shelf.title, summary?.counts(), appText = shelf !is Shelf.Search) {
            if (!empty) SelectButton(selection)
        }
        val held = summary ?: return@Column
        if (empty) {
            val (title, detail) = shelf.empty
            KoanUnavailable(title, shelf.icon, detail)
        } else {
            ShelfSections(shelf, held, selection, Modifier.weight(1f))
        }
        if (selection.active) SelectionBar(selection)
    }
}

/** Only the kinds there are, so a shelf of tracks reads as a count of tracks. */
private fun ShelfSummary.counts() = listOfNotNull(
    count(artistTotal.toLong(), "artist").takeIf { artistTotal > 0u },
    count(albumTotal.toLong(), "album").takeIf { albumTotal > 0u },
    count(trackTotal.toLong(), "track").takeIf { trackTotal > 0u },
).joinToString(" · ")

private val Shelf.icon
    get() = when (this) {
        Shelf.Favourites -> KoanIcons.favourite
        Shelf.Recent -> KoanIcons.recentlyPlayed
        Shelf.Downloaded -> KoanIcons.onDevice
        is Shelf.Search -> KoanIcons.search
    }

private val Shelf.empty
    get() = when (this) {
        Shelf.Favourites -> "No favourites yet" to "Tap the heart on a track, record or artist to keep it here."
        Shelf.Recent -> "Nothing played lately" to "What you play shows up here."
        Shelf.Downloaded -> "Nothing downloaded" to "Download a record from its menu to play it without a connection."
        is Shelf.Search -> "Nothing matches" to null
    }

/**
 * The sections of a shelf's page, each of whose artists, records and tracks
 * the select button can pick.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ShelfSections(shelf: Shelf, summary: ShelfSummary, selection: Selection<Playable>, modifier: Modifier = Modifier) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val gutter = LocalKoanTokens.current.gutter
    val (current, playing) = nowPlayingTrack()
    val trackIds = remember(summary) { summary.tracks.map { it.id } }
    val list = rememberLazyListState()
    BoxWithConstraints(modifier.fillMaxSize()) {
        // As many tiles across as fit at the grids' smallest size.
        val columns = ((maxWidth - gutter * 2 + 12.dp) / (150.dp + 12.dp)).toInt().coerceAtLeast(2)
        val rows = remember(summary, columns) { summary.albums.chunked(columns) }
        LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
            if (summary.artists.isNotEmpty()) {
                item(key = "artists-head") { SectionHead("Artists", summary.artistTotal.toLong(), { nav.push(ArtistsRoute(shelf)) }, Modifier.padding(horizontal = gutter)) }
                item(key = "artists") {
                    FlowRow(
                        Modifier.padding(horizontal = gutter),
                        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
                        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
                    ) {
                        summary.artists.forEach { ArtistPill(it.id, it.name, selection) }
                    }
                }
            }
            if (summary.albums.isNotEmpty()) {
                item(key = "albums-head") { SectionHead("Albums", summary.albumTotal.toLong(), { nav.push(AlbumsRoute(shelf)) }, Modifier.padding(horizontal = gutter)) }
                // A row of tiles per item, so the list recycles them as it
                // scrolls rather than laying out every sleeve at once.
                items(rows, key = { "albums-${it.first().id}" }) { row ->
                    Row(Modifier.fillMaxWidth().padding(horizontal = gutter - 4.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        row.forEach { AlbumTile(it, Modifier.weight(1f), selection = selection) }
                        repeat(columns - row.size) { Spacer(Modifier.weight(1f)) }
                    }
                }
            }
            if (summary.tracks.isNotEmpty()) {
                item(key = "tracks-head") { SectionHead("Tracks", summary.trackTotal.toLong(), { nav.push(TracksRoute(shelf)) }, Modifier.padding(horizontal = gutter)) }
                itemsIndexed(summary.tracks, key = { _, t -> "track-${t.id}" }) { index, track ->
                    TrackRow(
                        track,
                        lead = "${index + 1}",
                        current = track.id == current,
                        playing = playing,
                        onPlay = { app.playFrom(trackIds, index, nav) },
                        selection = selection,
                        mixed = true,
                    )
                }
            }
        }
    }
}

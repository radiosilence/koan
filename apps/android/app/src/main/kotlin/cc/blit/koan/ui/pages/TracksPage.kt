package cc.blit.koan.ui.pages

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import cc.blit.koan.state.activeCount
import cc.blit.koan.state.label
import cc.blit.koan.state.offeredTrackSorts
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import uniffi.koan_ffi.Track

private val RowSleeve = 48.dp

/**
 * Every track in the library (Section.Tracks), as `TrackBrowser` lists them.
 * The model reads the listing a page at a time and `trackTotal` is the whole
 * count from the first page on, so the header is right while rows fill in
 * behind. A tap replaces the queue with that track.
 */
@Composable
fun TracksPage(runtime: KoanRuntime) {
    val library = runtime.library
    val tab = rememberHomeTab(runtime)
    val tracks by library.visibleTracks.collectAsStateWithLifecycle()
    val total by library.trackTotal.collectAsStateWithLifecycle()
    val favourites by library.favouriteTrackIds.collectAsStateWithLifecycle()
    val filter by library.filter.collectAsStateWithLifecycle()
    val browse by library.browseFilter.collectAsStateWithLifecycle()
    val sort by library.trackSort.collectAsStateWithLifecycle()
    val signedIn by library.signedIn.collectAsStateWithLifecycle()
    val narrowed = filter.isNotEmpty() || browse.activeCount > 0
    val list = rememberRestoredListState(runtime, tab, Section.Tracks)
    val player = runtime.player
    val play = remember(player) { { id: Long -> player.playNow(listOf(id)) } }

    Column(Modifier.fillMaxSize()) {
        BrowseHeader(runtime, Section.Tracks, countLabel(total, "track")) {
            ChoiceStrip(
                options = offeredTrackSorts(browse.recent, filter.isNotEmpty()).map { it to it.label },
                selected = sort,
                onSelect = { it?.let(library::setTrackSort) },
            )
        }
        if (tracks.isEmpty()) {
            EmptyNote(
                "track",
                if (narrowed) "No matches" else "No tracks yet",
                if (narrowed) null else library.emptyLibraryDetail(signedIn),
            )
        } else {
            LazyColumn(Modifier.fillMaxSize(), state = list) {
                items(tracks, key = { it.id }, contentType = { "track" }) { track ->
                    TrackRow(track, track.id in favourites, play)
                }
            }
        }
    }
}

@Composable
fun TrackRow(track: Track, favourite: Boolean, onPlay: (Long) -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth()) {
        Row(
            Modifier.fillMaxWidth().sizeIn(minHeight = KoanShape.hit)
                .clickable(role = Role.Button) { onPlay(track.id) }
                .padding(horizontal = KoanSpace.l, vertical = KoanSpace.xs),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            val albumId = track.albumId
            if (albumId != null) Sleeve(ArtworkSource.Album(albumId), RowSleeve) else Box(Modifier.size(RowSleeve))
            Column(Modifier.weight(1f)) {
                KoanText(track.title, KoanRole.Body, KoanTone.Strong, data = true)
                KoanText(
                    listOf(track.artistName, track.albumTitle).filter { it.isNotEmpty() }.joinToString(" · "),
                    KoanRole.Fine, KoanTone.Muted, data = true,
                )
            }
            if (favourite) KoanIcon("heart-filled", tone = KoanTone.Accent, textSizeDp = 14f, description = "Favourite")
            KoanText(durationLabel(track.durationMs), KoanRole.Fine, KoanTone.Muted)
        }
        KoanRule(Modifier.padding(horizontal = KoanSpace.l))
    }
}

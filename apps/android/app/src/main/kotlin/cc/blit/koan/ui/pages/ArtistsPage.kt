package cc.blit.koan.ui.pages

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import cc.blit.koan.state.activeCount
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone
import uniffi.koan_ffi.Artist

/**
 * The artist list (Section.Artists), as `ArtistBrowser` lists them: a row a
 * name with how many records and tracks, a heart for a favourite, a tap
 * opening the artist. `library.visibleArtists` is the whole listing as the
 * database narrowed it.
 */
@Composable
fun ArtistsPage(runtime: KoanRuntime) {
    val library = runtime.library
    val tab = rememberHomeTab(runtime)
    val artists by library.visibleArtists.collectAsStateWithLifecycle()
    val favourites by library.favouriteArtistIds.collectAsStateWithLifecycle()
    val filter by library.filter.collectAsStateWithLifecycle()
    val browse by library.browseFilter.collectAsStateWithLifecycle()
    val signedIn by library.signedIn.collectAsStateWithLifecycle()
    val narrowed = filter.isNotEmpty() || browse.activeCount > 0
    val list = rememberRestoredListState(runtime, tab, Section.Artists)
    val navigator = runtime.navigator
    val open = remember(navigator) { { id: Long -> navigator.openArtist(id) } }

    Column(Modifier.fillMaxSize()) {
        BrowseHeader(runtime, Section.Artists, if (narrowed) countLabel(artists.size.toLong(), "artist") else null)
        if (artists.isEmpty()) {
            EmptyNote(
                "artist",
                if (narrowed) "Nothing matches" else "No artists yet",
                if (narrowed) "Try a different filter." else library.emptyLibraryDetail(signedIn),
            )
        } else {
            LazyColumn(Modifier.fillMaxSize(), state = list) {
                items(artists, key = { it.id }, contentType = { "artist" }) { artist ->
                    ArtistRow(artist, artist.id in favourites, open)
                }
            }
        }
    }
}

@Composable
fun ArtistRow(artist: Artist, favourite: Boolean, onOpen: (Long) -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth()) {
        Row(
            Modifier.fillMaxWidth().sizeIn(minHeight = KoanShape.hit + KoanSpace.s)
                .clickable(role = Role.Button) { onOpen(artist.id) }
                .padding(horizontal = KoanSpace.l, vertical = KoanSpace.s),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                KoanText(artist.name, KoanRole.Body, KoanTone.Strong, data = true)
                KoanText(
                    "${countLabel(artist.albumCount, "album")} · ${countLabel(artist.trackCount, "track")}",
                    KoanRole.Fine, KoanTone.Muted,
                )
            }
            if (favourite) KoanIcon("heart-filled", tone = KoanTone.Accent, textSizeDp = 14f, description = "Favourite")
        }
        KoanRule(Modifier.padding(horizontal = KoanSpace.l))
    }
}

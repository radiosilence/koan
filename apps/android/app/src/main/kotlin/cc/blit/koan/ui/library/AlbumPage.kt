package cc.blit.koan.ui.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Button
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.R
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage

/**
 * A record: its sleeve and what it is, play and shuffle, and its tracks,
 * grouped by disc when there is more than one. Playing a track plays the
 * record from there and leaves the page where it is, since the record is on
 * screen.
 */
@Composable
fun AlbumPage(albumId: Long) {
    val app = koanApp()
    val model = viewModel { AlbumModel(app.mirror, albumId) }
    val page by model.page.collectAsStateWithLifecycle()
    val held = page
    val album = held?.album
    when {
        held == null -> Column { PageBar("") }
        album == null -> Column {
            PageBar("")
            EmptyState("This album isn't in the library any more")
        }
        else -> RecordAccent(albumId) { Record(album, held.tracks) }
    }
}

@Composable
private fun Record(album: Album, tracks: List<Track>) {
    val app = koanApp()
    val (current, playing) = nowPlayingTrack()
    val ids = remember(tracks) { tracks.map { it.id } }
    val discs = remember(tracks) { tracks.withIndex().groupBy { it.value.disc ?: 1 }.toSortedMap() }
    val list = rememberLazyListState()
    Column {
        // The title moves into the bar once the header has scrolled away.
        val scrolled by remember { derivedStateOf { list.firstVisibleItemIndex > 0 } }
        PageBar(if (scrolled) album.title else "")
        LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
            item(key = "header") { Header(album, tracks) }
            discs.forEach { (disc, rows) ->
                if (discs.size > 1) {
                    item(key = "disc-$disc") {
                        Text(
                            "Disc $disc",
                            style = MaterialTheme.typography.titleSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = LocalKoanTokens.current.gutter, vertical = 12.dp),
                        )
                    }
                }
                itemsIndexed(rows, key = { _, row -> row.value.id }, contentType = { _, _ -> "track" }) { _, (index, track) ->
                    TrackRow(
                        track,
                        lead = track.trackNumber?.toString() ?: "${index + 1}",
                        current = track.id == current,
                        playing = playing,
                        onPlay = { app.player.playTracks(ids, index) },
                    )
                }
            }
        }
    }
}

@Composable
private fun Header(album: Album, tracks: List<Track>) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val gutter = LocalKoanTokens.current.gutter
    var menu by remember { mutableStateOf(false) }
    val playable = Playable.OfAlbum(album)
    Column(Modifier.fillMaxWidth().padding(horizontal = gutter), horizontalAlignment = Alignment.CenterHorizontally) {
        AsyncImage(
            Art.album(album.id, ArtSize.Full),
            null,
            Modifier.widthIn(max = 320.dp).fillMaxWidth(0.72f).aspectRatio(1f).padding(bottom = 16.dp),
        )
        Text(album.title, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center, maxLines = 3, overflow = TextOverflow.Ellipsis)
        Text(
            album.artistName,
            style = MaterialTheme.typography.titleMedium,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.clickable { nav.push(ArtistRoute(album.artistId)) }.padding(vertical = 2.dp),
        )
        val total = tracks.sumOf { it.durationMs ?: 0 }
        Text(
            listOfNotNull(album.year?.toString(), album.codec?.uppercase(), length(total).takeIf { total > 0 }).joinToString(" · "),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Row(
            Modifier.padding(vertical = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Button(onClick = { app.play(playable) }) {
                Icon(painterResource(R.drawable.ic_play), null)
                Text("Play", Modifier.padding(start = 6.dp))
            }
            FilledTonalButton(onClick = { app.play(playable, shuffled = true) }) {
                Icon(painterResource(R.drawable.ic_shuffle), null)
                Text("Shuffle", Modifier.padding(start = 6.dp))
            }
            AlbumHeart(album.id)
            Box {
                IconButton(onClick = { menu = true }) { Icon(painterResource(R.drawable.ic_more), "More") }
                PlayableMenu(playable, menu) { menu = false }
            }
        }
    }
}

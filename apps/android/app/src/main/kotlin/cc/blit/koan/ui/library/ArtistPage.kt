package cc.blit.koan.ui.library

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.R
import cc.blit.koan.art.Art
import cc.blit.koan.ffi.Artist
import cc.blit.koan.ffi.ArtistInfo
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage

/**
 * An artist: their photograph and name, play and shuffle, the opening of their
 * biography, and their records as a grid, since that is how a discography is
 * thought of. An artist credited only on other people's records gets those
 * tracks instead of an empty grid.
 */
@Composable
fun ArtistPage(artistId: Long) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val model = viewModel { ArtistModel(app.mirror, artistId) }
    val record by model.record.collectAsStateWithLifecycle()
    val info by model.info.collectAsStateWithLifecycle()
    val looking by model.looking.collectAsStateWithLifecycle()
    val gutter = LocalKoanTokens.current.gutter
    val grid = rememberLazyGridState()
    val list = rememberLazyListState()
    val (current, playing) = nowPlayingTrack()
    val held = record
    // The name moves into the bar once the header has scrolled away.
    val scrolled by remember { derivedStateOf { grid.firstVisibleItemIndex > 0 || list.firstVisibleItemIndex > 0 } }
    Column {
        PageBar(if (scrolled) held?.artist?.name.orEmpty() else "")
        if (held == null) return@Column
        val artist = held.artist ?: return@Column EmptyState("This artist isn't in the library any more")
        if (held.albums.isEmpty() && held.appearances.isNotEmpty()) {
            val ids = remember(held) { held.appearances.map { it.id } }
            val records = held.appearances.mapNotNull { it.albumId }.toSet().size
            LazyColumn(state = list, modifier = Modifier.fillMaxSize()) {
                item(key = "header") { Header(artistId, artist, "Appears on ${count(records.toLong(), "album")}", info, looking) }
                itemsIndexed(held.appearances, key = { _, t -> t.id }) { index, track ->
                    TrackRow(
                        track,
                        lead = "${index + 1}",
                        current = track.id == current,
                        playing = playing,
                        onPlay = { app.playFrom(ids, index, nav) },
                        mixed = true,
                    )
                }
            }
        } else {
            LazyVerticalGrid(
                columns = GridCells.Adaptive(150.dp),
                state = grid,
                contentPadding = PaddingValues(start = gutter - 4.dp, end = gutter - 4.dp, bottom = 16.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                item(key = "header", span = { GridItemSpan(maxLineSpan) }) {
                    Header(artistId, artist, count(held.albums.size.toLong(), "album"), info, looking)
                }
                items(held.albums, key = { it.id }, contentType = { "album" }) { AlbumTile(it, showArtist = false) }
            }
        }
    }
}

@Composable
private fun Header(artistId: Long, artist: Artist, counts: String, info: ArtistInfo?, looking: Boolean) {
    val app = koanApp()
    var menu by remember { mutableStateOf(false) }
    val playable = Playable.OfArtist(artist.id, artist.name)
    Column(Modifier.fillMaxWidth().padding(horizontal = 4.dp).animateContentSize()) {
        AnimatedVisibility(info?.hasImage == true) {
            AsyncImage(
                Art.artist(artistId),
                null,
                Modifier
                    .padding(bottom = 12.dp)
                    .size(160.dp)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.surfaceVariant),
                contentScale = ContentScale.Crop,
            )
        }
        Text(artist.name, style = MaterialTheme.typography.headlineSmall, maxLines = 3, overflow = TextOverflow.Ellipsis)
        Text(counts, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
            ArtistHeart(artist.id)
            Box {
                IconButton(onClick = { menu = true }) { Icon(painterResource(R.drawable.ic_more), "More") }
                PlayableMenu(playable, menu) { menu = false }
            }
        }
        val bio = info?.bio
        when {
            bio != null -> Bio(bio, info.bioUrl, info.imageCredit)
            looking -> Row(Modifier.padding(bottom = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                Text(
                    "Looking up biography…",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(start = 8.dp),
                )
            }
        }
    }
}

/** The opening of the artist's Wikipedia article: a few lines, a tap for the rest, credited as its licence asks. */
@Composable
private fun Bio(bio: String, source: String?, imageCredit: String?) {
    var expanded by rememberSaveable { mutableStateOf(false) }
    val uri = LocalUriHandler.current
    Column(Modifier.padding(bottom = 12.dp).clickable { expanded = !expanded }) {
        Text(
            // The extract separates paragraphs with a single newline.
            bio.replace("\n", "\n\n"),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = if (expanded) Int.MAX_VALUE else 4,
            overflow = TextOverflow.Ellipsis,
        )
        if (expanded) {
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                if (source != null) {
                    Text(
                        "From Wikipedia",
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.clickable { uri.openUri(source) },
                    )
                }
                if (imageCredit != null) {
                    Text("Photo: $imageCredit", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        } else {
            Text("more", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(top = 4.dp))
        }
    }
}

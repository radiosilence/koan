package cc.blit.koan.ui.search

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.Checkbox
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import cc.blit.koan.R
import cc.blit.koan.art.Art
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.library.ArtistRoute
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage

/** The narrowest a record tile goes, as the Apple apps' result grid. */
internal val TileMinimum = 140.dp

/** A long press starts picking with the thing pressed in the pick, as Android lists do. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Modifier.pickable(playable: Playable, selection: Selection, open: () -> Unit): Modifier {
    val haptics = LocalHapticFeedback.current
    return combinedClickable(
        onClick = { if (!selection.take(playable)) open() },
        onLongClick = {
            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
            if (selection.active) selection.toggle(playable) else selection.begin(playable)
        },
    )
}

/** A section's name, how many the library has, and a chevron: the whole of it opens the listing. */
@Composable
internal fun SectionHeading(title: String, total: ULong, open: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = open).padding(vertical = LocalKoanTokens.current.rowGap),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(LocalKoanTokens.current.rowGap),
    ) {
        Text(title, style = MaterialTheme.typography.titleMedium)
        Text("$total", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Icon(painterResource(R.drawable.ic_chevron_right), "See all", tint = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
internal fun ArtistPill(artist: Playable.Artist, selection: Selection) {
    val nav = LocalNavigator.current
    // A chip draws its own click; the pick's long press goes on a box over it.
    Box {
        FilterChip(
            selected = selection.contains(artist),
            onClick = {},
            label = { Text(artist.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            modifier = Modifier.widthIn(max = 260.dp),
        )
        Box(Modifier.matchParentSize().pickable(artist, selection) { nav.push(ArtistRoute(artist.id)) })
    }
}

/**
 * A record: the sleeve plays it, the title opens it, the artist goes to the
 * artist. While picking, the whole tile ticks.
 */
@Composable
internal fun AlbumTile(album: Album, selection: Selection, onSleeve: () -> Unit) {
    val nav = LocalNavigator.current
    val playable = Playable.Record(album)
    val picked = selection.contains(playable)
    val shape = MaterialTheme.shapes.small
    Column {
        Box(
            Modifier.fillMaxWidth().aspectRatio(1f).clip(shape)
                .then(if (picked) Modifier.border(BorderStroke(3.dp, MaterialTheme.colorScheme.primary), shape) else Modifier)
                .pickable(playable, selection, onSleeve),
        ) {
            AsyncImage(Art.album(album.id), album.title, Modifier.matchParentSize(), contentScale = ContentScale.Crop)
            if (selection.active) {
                Checkbox(checked = picked, onCheckedChange = null, modifier = Modifier.align(Alignment.TopStart).padding(4.dp))
            }
        }
        Text(
            album.title,
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.fillMaxWidth().pickable(playable, selection) { nav.push(AlbumRoute(album.id)) },
        )
        Text(
            listOfNotNull(album.artistName, album.year?.toString()).joinToString(" · "),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.fillMaxWidth().pickable(playable, selection) { nav.push(ArtistRoute(album.artistId)) },
        )
    }
}

/** A track points at its record: that is where it lives, and where it is played from. */
@Composable
internal fun TrackRow(track: Track, selection: Selection) {
    val nav = LocalNavigator.current
    val playable = Playable.Song(track)
    ListItem(
        headlineContent = { Text(track.title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        supportingContent = {
            Text("${track.artistName} — ${track.albumTitle}", maxLines = 1, overflow = TextOverflow.Ellipsis)
        },
        leadingContent = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (selection.active) Checkbox(checked = selection.contains(playable), onCheckedChange = null)
                AsyncImage(
                    track.albumId?.let { Art.album(it) } ?: Art.track(track.id),
                    null,
                    Modifier.size(LocalKoanTokens.current.thumb).clip(MaterialTheme.shapes.extraSmall),
                )
            }
        },
        trailingContent = { track.durationMs?.let { Text(duration(it), style = MaterialTheme.typography.labelMedium) } },
        colors = ListItemDefaults.colors(containerColor = androidx.compose.ui.graphics.Color.Transparent),
        modifier = Modifier.pickable(playable, selection) { track.albumId?.let { nav.push(AlbumRoute(it)) } },
    )
}

private fun duration(ms: Long): String {
    val s = ms / 1000
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

/**
 * What to do with the pick, at the foot of the page. Each verb resolves the
 * pick to tracks in the order it was made, acts, and ends the mode.
 */
@Composable
internal fun SelectionBar(selection: Selection) {
    val app = koanApp()
    val count = selection.picked.size
    fun run(action: suspend (List<Long>) -> Unit) {
        val items = selection.picked.toList()
        selection.end()
        app.mirror.launch {
            val ids = items.flatMap { it.trackIds(this) }
            if (ids.isNotEmpty()) action(ids)
        }
    }
    Surface(tonalElevation = 3.dp) {
        Column(Modifier.fillMaxWidth().padding(horizontal = LocalKoanTokens.current.gutter)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    if (count == 0) "Select items" else "$count selected",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = selection::end) { Text("Done") }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                TextButton(enabled = count > 0, onClick = { run { app.player.playTracks(it) } }) { Text("Play") }
                TextButton(enabled = count > 0, onClick = { run { app.player.playNext(it) } }) { Text("Next") }
                TextButton(enabled = count > 0, onClick = { run { app.player.enqueue(it) } }) { Text("Queue") }
                TextButton(enabled = count > 0, onClick = { favourite(app, selection) }) { Text("Favourite") }
            }
        }
    }
}

/**
 * Hearts every one of the pick, or, where all of them are hearted already,
 * takes the hearts off: the things themselves, not the tracks behind them.
 */
private fun favourite(app: cc.blit.koan.KoanApp, selection: Selection) {
    val items = selection.picked.toList()
    selection.end()
    app.mirror.launch {
        val tracks = favouriteTrackIds().toSet()
        val albums = favouriteAlbumIds().toSet()
        val artists = favouriteArtistIds().toSet()
        fun hearted(item: Playable) = when (item) {
            is Playable.Song -> item.track.id in tracks
            is Playable.Record -> item.album.id in albums
            is Playable.Artist -> item.id in artists
        }
        val all = items.all(::hearted)
        for (item in items) {
            if (hearted(item) != all) continue
            when (item) {
                is Playable.Song -> toggleFavourite(item.track.id)
                is Playable.Record -> toggleFavouriteAlbum(item.album.id)
                is Playable.Artist -> toggleFavouriteArtist(item.id)
            }
        }
    }
}

package cc.blit.koan.ui.library

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import cc.blit.koan.R
import cc.blit.koan.art.Art
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.Artist
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.LocalKoanTokens
import coil3.compose.AsyncImage

/** `3:07`, or `1:02:45` past an hour. */
fun duration(ms: Long): String {
    val s = ms / 1000
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

/** `42 min`, or `1 h 12 min`: a record's length. */
fun length(ms: Long): String {
    val m = (ms + 30_000) / 60_000
    return if (m >= 60) "${m / 60} h ${m % 60} min" else "$m min"
}

fun count(n: Long, noun: String) = "$n ${if (n == 1L) noun else "${noun}s"}"

/** A pushed page's bar: back, the title, a line under it, and the page's actions. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PageBar(title: String, subtitle: String? = null, back: Boolean = true, actions: @Composable () -> Unit = {}) {
    val nav = LocalNavigator.current
    TopAppBar(
        title = {
            Column {
                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (!subtitle.isNullOrEmpty()) {
                    Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        },
        navigationIcon = {
            if (back) IconButton(onClick = { nav.pop() }) { Icon(painterResource(R.drawable.ic_back), "Back") }
        },
        actions = { actions() },
    )
}

/** What an empty page says, centred. */
@Composable
fun EmptyState(title: String, detail: String? = null, modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxSize().padding(LocalKoanTokens.current.gutter * 2),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
        if (detail != null) {
            Text(
                detail,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(top = 8.dp),
            )
        }
    }
}

@Composable
fun Sleeve(albumId: Long?, modifier: Modifier = Modifier, corner: Int = 6) {
    val shape = RoundedCornerShape(corner.dp)
    val ground = Modifier.clip(shape).background(MaterialTheme.colorScheme.surfaceVariant)
    if (albumId == null) {
        Box(modifier.then(ground))
    } else {
        AsyncImage(Art.album(albumId), null, modifier.then(ground), contentScale = ContentScale.Crop)
    }
}

/**
 * One record in a grid: the sleeve with its codec, the title, and the artist
 * and year. A tap opens it; a long press offers the rest.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun AlbumTile(album: Album, modifier: Modifier = Modifier, showArtist: Boolean = true) {
    val nav = LocalNavigator.current
    var menu by remember { mutableStateOf(false) }
    Column(
        modifier
            .clip(RoundedCornerShape(8.dp))
            .combinedClickable(onClick = { nav.push(AlbumRoute(album.id)) }, onLongClick = { menu = true })
            .padding(4.dp),
    ) {
        Box {
            Sleeve(album.id, Modifier.fillMaxWidth().aspectRatio(1f), corner = 8)
            album.codec?.let { codec ->
                Text(
                    codec.uppercase(),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurface,
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .padding(6.dp)
                        .clip(RoundedCornerShape(50))
                        .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.7f))
                        .padding(horizontal = 6.dp, vertical = 2.dp),
                )
            }
            album.onDevice?.let { on ->
                if (on.total > 0u) DownloadedBar(on.have.toFloat() / on.total.toFloat(), Modifier.align(Alignment.BottomStart))
            }
            AlbumHeart(album.id, Modifier.align(Alignment.BottomEnd), showing = false)
            PlayableMenu(Playable.OfAlbum(album), menu) { menu = false }
        }
        Text(
            album.title,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.Medium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 6.dp),
        )
        Text(
            listOfNotNull(album.artistName.takeIf { showArtist }, album.year?.toString()).joinToString(" · "),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** How much of a record is on this device, as a bar along the foot of its sleeve. */
@Composable
private fun DownloadedBar(fraction: Float, modifier: Modifier = Modifier) {
    Box(
        modifier
            .padding(start = 8.dp, end = 44.dp, bottom = 8.dp)
            .fillMaxWidth()
            .height(3.dp)
            .clip(RoundedCornerShape(50))
            .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.6f)),
    ) {
        Box(Modifier.fillMaxWidth(fraction).fillMaxHeight().background(MaterialTheme.colorScheme.primary))
    }
}

/** A record as a row, for the browser's list layout. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun AlbumRow(album: Album, modifier: Modifier = Modifier) {
    val nav = LocalNavigator.current
    val tokens = LocalKoanTokens.current
    var menu by remember { mutableStateOf(false) }
    Row(
        modifier
            .fillMaxWidth()
            .combinedClickable(onClick = { nav.push(AlbumRoute(album.id)) }, onLongClick = { menu = true })
            .padding(horizontal = tokens.gutter, vertical = tokens.rowGap / 2),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Sleeve(album.id, Modifier.size(tokens.thumb))
        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Text(album.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge)
            Text(
                listOfNotNull(album.artistName, album.year?.toString(), album.codec?.uppercase()).joinToString(" · "),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        AlbumHeart(album.id, showing = false)
        Box { PlayableMenu(Playable.OfAlbum(album), menu) { menu = false } }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ArtistRow(artist: Artist, modifier: Modifier = Modifier) {
    val nav = LocalNavigator.current
    val tokens = LocalKoanTokens.current
    var menu by remember { mutableStateOf(false) }
    Row(
        modifier
            .fillMaxWidth()
            .combinedClickable(onClick = { nav.push(ArtistRoute(artist.id)) }, onLongClick = { menu = true })
            .padding(horizontal = tokens.gutter, vertical = tokens.rowGap),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(artist.name, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge)
            Text(
                listOfNotNull(
                    count(artist.albumCount, "album").takeIf { artist.albumCount > 0 },
                    count(artist.trackCount, "track"),
                ).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        ArtistHeart(artist.id, showing = false)
        Box { PlayableMenu(Playable.OfArtist(artist.id, artist.name), menu) { menu = false } }
    }
}

/**
 * One track. Its number, or bars while it is the one playing; the title, and
 * the artist where it is not the record's. Gathered from all over
 * (`mixed`), a row carries its own sleeve and names its record.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun TrackRow(
    track: Track,
    lead: String,
    current: Boolean,
    playing: Boolean,
    onPlay: () -> Unit,
    modifier: Modifier = Modifier,
    mixed: Boolean = false,
) {
    val tokens = LocalKoanTokens.current
    var menu by remember { mutableStateOf(false) }
    val accent = MaterialTheme.colorScheme.primary
    Row(
        modifier
            .fillMaxWidth()
            .combinedClickable(onClick = onPlay, onLongClick = { menu = true })
            .padding(start = tokens.gutter, end = 4.dp, top = tokens.rowGap / 2, bottom = tokens.rowGap / 2),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (mixed) {
            Box(Modifier.size(tokens.thumb), contentAlignment = Alignment.Center) {
                Sleeve(track.albumId, Modifier.fillMaxSize())
                if (current) {
                    Box(Modifier.fillMaxSize().clip(RoundedCornerShape(6.dp)).background(MaterialTheme.colorScheme.scrim.copy(alpha = 0.45f)))
                    PlayingBars(playing, color = MaterialTheme.colorScheme.inverseOnSurface)
                }
            }
        } else {
            Box(Modifier.width(28.dp), contentAlignment = Alignment.CenterStart) {
                if (current) {
                    PlayingBars(playing)
                } else {
                    Text(lead, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Text(
                track.title,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodyLarge,
                color = if (current) accent else MaterialTheme.colorScheme.onSurface,
            )
            val under = if (mixed) "${track.artistName} · ${track.albumTitle}" else track.artistName.takeIf { it != track.albumArtistName }
            if (under != null) {
                Text(
                    under,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        TrackHeart(track.id, showing = false)
        track.durationMs?.let {
            Text(duration(it), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Box { PlayableMenu(Playable.OfTrack(track), menu) { menu = false } }
    }
}

/** A heading in a page of sections, opening the section's whole listing. */
@Composable
fun SectionHead(title: String, total: Long, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    val tokens = LocalKoanTokens.current
    Row(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .clickable(onClick = onOpen)
            .padding(horizontal = tokens.gutter, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, style = MaterialTheme.typography.titleMedium)
        Text(
            "$total",
            style = MaterialTheme.typography.titleMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(start = 8.dp),
        )
        Icon(painterResource(R.drawable.ic_chevron_right), "See all", Modifier.padding(start = 2.dp).size(20.dp))
    }
}

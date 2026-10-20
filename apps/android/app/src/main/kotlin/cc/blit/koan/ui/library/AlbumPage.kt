package cc.blit.koan.ui.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens

/**
 * A record: its sleeve and what it is, play and shuffle, and its tracks,
 * grouped by disc when there is more than one. Playing a track plays the
 * record from there and leaves the page where it is, since the record is on
 * screen. The select button picks tracks to play or queue together.
 */
@Composable
fun AlbumPage(albumId: Long, track: Long? = null) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val model = viewModel { AlbumModel({ app.mirror.engine().albumPage(albumId) }, app.mirror.library) }
    val read by model.page.collectAsStateWithLifecycle()
    val album = (read as? AlbumRead.Read)?.page?.album
    when (val held = read) {
        AlbumRead.Reading -> PageBar("")
        is AlbumRead.Read if album != null -> RecordAccent(albumId) { Record(album, held.page.tracks, track, model.selection) }
        // Never a record page with nothing on it: gone from the library, or
        // the read failed, each said as such. A failed read is worth trying
        // again; a record that has gone comes back, if it does, with the library.
        else -> Column {
            PageBar("")
            val failed = held == AlbumRead.Failed
            KoanUnavailable(
                if (failed) "Couldn't read this album" else "This album isn't in the library any more",
                KoanIcons.unknownAlbum,
                null,
            ) {
                KoanButton("Go Back", { nav.pop() })
                if (failed) KoanButton("Try Again", model::retry)
            }
        }
    }
}

/** What a record's page lists, in order: the header, then each disc's heading where there are several, and its tracks. */
sealed interface RecordItem {
    data object Heading : RecordItem

    data class Disc(val number: Int) : RecordItem

    /** A track, and its place in the record's running order. */
    data class Line(val index: Int, val track: Track) : RecordItem
}

fun recordItems(tracks: List<Track>): List<RecordItem> {
    val discs = tracks.mapIndexed(RecordItem::Line).groupBy { it.track.disc ?: 1 }.toSortedMap()
    return listOf(RecordItem.Heading) + discs.flatMap { (disc, lines) ->
        (if (discs.size > 1) listOf(RecordItem.Disc(disc)) else emptyList()) + lines
    }
}

@Composable
private fun Record(album: Album, tracks: List<Track>, reveal: Long?, selection: Selection<Playable>) {
    val app = koanApp()
    SelectMode(selection)
    val (current, playing) = nowPlayingTrack()
    val ids = remember(tracks) { tracks.map { it.id } }
    val items = remember(tracks) { recordItems(tracks) }
    val list = rememberLazyListState()
    // Opened from one of its tracks: that track is marked, and brought into
    // view once, so coming back to the page leaves it where it was left.
    var revealed by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(items) {
        if (revealed || reveal == null) return@LaunchedEffect
        revealed = true
        val at = items.indexOfFirst { it is RecordItem.Line && it.track.id == reveal }
        if (at > 0) list.scrollToItem(at, -RevealOffsetPx)
    }
    // The title moves into the bar once the header has scrolled away.
    val scrolled by remember { derivedStateOf { list.firstVisibleItemIndex > 0 } }
    Column {
        PageBar(if (scrolled) album.title else "", appText = false) {
            if (tracks.isNotEmpty()) SelectButton(selection)
        }
        LazyColumn(state = list, modifier = Modifier.weight(1f)) {
            items(
                items,
                key = {
                    when (it) {
                        RecordItem.Heading -> "header"
                        is RecordItem.Disc -> "disc-${it.number}"
                        is RecordItem.Line -> it.track.id
                    }
                },
                contentType = { it::class },
            ) { item ->
                when (item) {
                    RecordItem.Heading -> Header(album, tracks)
                    is RecordItem.Disc ->
                        KoanSectionHeader("Disc ${item.number}", Modifier.padding(horizontal = LocalKoanTokens.current.gutter))
                    is RecordItem.Line -> TrackRow(
                        item.track,
                        lead = item.track.trackNumber?.toString() ?: "${item.index + 1}",
                        current = item.track.id == current,
                        playing = playing,
                        onPlay = { app.player.playTracks(ids, item.index) },
                        selection = selection,
                        marked = item.track.id == reveal,
                    )
                }
            }
        }
        if (selection.active) SelectionBar(selection)
    }
}

/** How far above a revealed track the list stops, so the rows before it show it in context. */
private const val RevealOffsetPx = 160

@Composable
private fun Header(album: Album, tracks: List<Track>) {
    val app = koanApp()
    var menu by remember { mutableStateOf(false) }
    var viewing by remember { mutableStateOf(false) }
    val playable = Playable.OfAlbum(album)
    val total = tracks.sumOf { it.durationMs ?: 0 }
    val details = listOfNotNull(album.year?.toString(), album.codec?.uppercase(), length(total).takeIf { total > 0 }).joinToString(" · ")
    val credit = artistCredit(album.artistCredit, album.artistName, album.artistId)
    PageHeader(
        album.title,
        Modifier.padding(horizontal = LocalKoanTokens.current.gutter),
        art = {
            Sleeve(
                Art.album(album.id, ArtSize.Full),
                Modifier.clickable(remember { MutableInteractionSource() }, indication = null, onClickLabel = "Show artwork") { viewing = true },
                size = 200.dp,
            )
        },
        detail = {
            NameLine(listOf(credit), maxLines = 2)
            if (details.isNotEmpty()) KoanText(details, role = KoanType.Fine, tone = KoanTone.Muted)
        },
        actions = {
            KoanButton("Play", { app.play(playable) }, kind = KoanButtonKind.Prominent, icon = KoanIcons.play)
            KoanButton("Shuffle", { app.play(playable, shuffled = true) }, icon = KoanIcons.shuffle)
            AlbumHeart(album.id, labelled = true)
            Box {
                KoanButton("More", { menu = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.more)
                PlayableMenu(playable, menu) { menu = false }
            }
        },
    )
    if (viewing) {
        ArtworkViewer(Art.album(album.id, ArtSize.Full), album.title, listOf(credit.text, details).filter { it.isNotEmpty() }.joinToString(" · ")) { viewing = false }
    }
}

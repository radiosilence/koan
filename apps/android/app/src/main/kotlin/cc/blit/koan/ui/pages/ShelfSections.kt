package cc.blit.koan.ui.pages

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.state.Pick
import cc.blit.koan.state.Playable
import cc.blit.koan.state.PlayableSelection
import cc.blit.koan.state.ShelfTotals
import cc.blit.koan.state.headingCount
import cc.blit.koan.state.rowsOf
import cc.blit.koan.state.tileColumns
import cc.blit.koan.state.ShelfList
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanIconButton
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import uniffi.koan_ffi.Album
import uniffi.koan_ffi.Artist
import uniffi.koan_ffi.Track

/**
 * What a results page does with a tap, as plain callbacks: the page binds
 * them to the models, so nothing below reaches the engine.
 */
class ShelfActions(
    val openSection: (ShelfList) -> Unit,
    val openArtist: (Long) -> Unit,
    val openAlbum: (Long) -> Unit,
    /** A track tapped: where it is in the shown tracks, and the ids of all of them. */
    val tapTrack: (Track, Int) -> Unit,
    val toggleFavourite: (Track) -> Unit,
)

/**
 * Artists as pills, records as tiles and tracks as rows, in one lazy list:
 * what favourites, recently played, on device and search all show (the port of
 * the Mac's `MixedCollection`). They are the head of the listings their
 * headings open, with the count of those listings.
 *
 * A record row of tiles is one list item rather than a grid inside an item, so
 * the list builds, and asks for sleeves for, the rows on screen and not every
 * record on the page.
 */
@OptIn(ExperimentalFoundationApi::class, ExperimentalLayoutApi::class)
@Composable
fun ShelfResults(
    artists: List<Artist>,
    albums: List<Album>,
    tracks: List<Track>,
    totals: ShelfTotals?,
    favouriteTracks: Set<Long>,
    pick: Pick,
    selection: PlayableSelection,
    actions: ShelfActions,
    modifier: Modifier = Modifier,
    header: (@Composable () -> Unit)? = null,
) {
    BoxWithConstraints(modifier.fillMaxSize()) {
        val columns = remember(maxWidth) {
            tileColumns((maxWidth - KoanSpace.page * 2).value)
        }
        val albumRows = remember(albums, columns) { rowsOf(albums, columns) }
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = PaddingValues(horizontal = KoanSpace.page, vertical = KoanSpace.l),
            verticalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            if (header != null) item(key = "header", contentType = "header") { header() }
            if (artists.isNotEmpty()) {
                item(key = "head-artists", contentType = "heading") {
                    SectionHeading("artists", headingCount(totals?.artists, artists.size)) {
                        actions.openSection(ShelfList.Artists)
                    }
                }
                item(key = "artists", contentType = "pills") {
                    FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
                        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
                    ) {
                        for (artist in artists) {
                            ArtistPill(artist, pick, selection, actions.openArtist)
                        }
                    }
                }
            }
            if (albums.isNotEmpty()) {
                item(key = "head-albums", contentType = "heading") {
                    SectionHeading("albums", headingCount(totals?.albums, albums.size)) {
                        actions.openSection(ShelfList.Albums)
                    }
                }
                items(albumRows, key = { "albums-${it.first().id}" }, contentType = { "tiles" }) { row ->
                    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.l)) {
                        for (album in row) {
                            AlbumTile(album, pick, selection, actions.openAlbum, Modifier.weight(1f))
                        }
                        // A short last row keeps the other rows' columns.
                        repeat(columns - row.size) { Spacer(Modifier.weight(1f)) }
                    }
                }
            }
            if (tracks.isNotEmpty()) {
                item(key = "head-tracks", contentType = "heading") {
                    SectionHeading("tracks", headingCount(totals?.tracks, tracks.size)) {
                        actions.openSection(ShelfList.Tracks)
                    }
                }
                items(tracks, key = { "track-${it.id}" }, contentType = { "track" }) { track ->
                    TrackResultRow(
                        track = track,
                        favourite = track.id in favouriteTracks,
                        pick = pick,
                        selection = selection,
                        onTap = { actions.tapTrack(track, tracks.indexOfFirst { it.id == track.id }) },
                        onToggleFavourite = { actions.toggleFavourite(track) },
                    )
                }
            }
        }
    }
}

/** A section's heading: its name, how many the library has, and a chevron, the whole of it opening the listing. */
@Composable
private fun SectionHeading(title: String, count: Long, open: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .height(KoanShape.hit)
            .clickable(role = Role.Button, onClick = open),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        KoanText(title, KoanRole.TitleSm, KoanTone.Strong)
        KoanText(count.toString(), KoanRole.Fine, KoanTone.Muted)
        KoanIcon("chevron-right", tone = KoanTone.Muted)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ArtistPill(artist: Artist, pick: Pick, selection: PlayableSelection, open: (Long) -> Unit) {
    val playable = remember(artist) { Playable.OfArtist(artist.id, artist.name) }
    val picked = playable.key in pick
    val colors = KoanTheme.colors
    Row(
        Modifier
            .border(KoanShape.hairline, if (picked) colors.accent else colors.rule, RectangleShape)
            .combinedClickable(
                role = Role.Button,
                onLongClick = { if (!pick.active) selection.begin(playable) },
                onClick = { if (!selection.take(playable)) open(artist.id) },
            )
            .semantics { selected = picked }
            .padding(horizontal = KoanSpace.m, vertical = KoanSpace.s),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        if (pick.active) Tick(picked)
        KoanIcon("artist", tone = KoanTone.Muted)
        KoanText(artist.name, KoanRole.Control, data = true)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun AlbumTile(
    album: Album,
    pick: Pick,
    selection: PlayableSelection,
    open: (Long) -> Unit,
    modifier: Modifier = Modifier,
) {
    val playable = remember(album) { Playable.OfAlbum(album) }
    val picked = playable.key in pick
    Column(
        modifier
            .combinedClickable(
                role = Role.Button,
                onLongClick = { if (!pick.active) selection.begin(playable) },
                onClick = { if (!selection.take(playable)) open(album.id) },
            )
            .semantics { selected = picked },
        verticalArrangement = Arrangement.spacedBy(KoanSpace.xs),
    ) {
        BoxWithConstraints(Modifier.fillMaxWidth()) {
            Sleeve(ArtworkSource.Album(album.id), maxWidth)
            if (pick.active) Tick(picked, Modifier.align(Alignment.TopStart).padding(KoanSpace.s))
        }
        KoanText(album.title, KoanRole.Control, KoanTone.Strong, data = true)
        KoanText(album.artistName, KoanRole.Fine, KoanTone.Muted, data = true)
        // On the downloaded shelf: how much of the record is on this device.
        album.onDevice?.let { KoanText("${it.have} of ${it.total} here", KoanRole.Fine, KoanTone.Muted) }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun TrackResultRow(
    track: Track,
    favourite: Boolean,
    pick: Pick,
    selection: PlayableSelection,
    onTap: () -> Unit,
    onToggleFavourite: () -> Unit,
) {
    val playable = remember(track) { Playable.OfTrack(track) }
    val picked = playable.key in pick
    Row(
        Modifier
            .fillMaxWidth()
            .combinedClickable(
                role = Role.Button,
                onLongClick = { if (!pick.active) selection.begin(playable) },
                onClick = { if (!selection.take(playable)) onTap() },
            )
            .semantics { selected = picked },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        if (pick.active) Tick(picked)
        // The cover is what you recognise a track by, and a results list is exactly where you are trying to.
        if (track.albumId != null) Sleeve(ArtworkSource.Album(track.albumId!!), 44.dp) else Box(Modifier.size(44.dp))
        Column(Modifier.weight(1f)) {
            KoanText(track.title, KoanRole.Body, data = true)
            KoanText("${track.artistName} — ${track.albumTitle}", KoanRole.Fine, KoanTone.Muted, data = true)
        }
        KoanText(shelfDuration(track.durationMs), KoanRole.Fine, KoanTone.Muted)
        if (!pick.active) {
            KoanIconButton(
                icon = if (favourite) "heart-filled" else "heart",
                description = if (favourite) "Remove from favourites" else "Add to favourites",
                onClick = onToggleFavourite,
                tone = if (favourite) KoanTone.Accent else KoanTone.Muted,
            )
        }
    }
}

/** The tick a picked item wears while picking: a square, filled in the accent when ticked. */
@Composable
private fun Tick(picked: Boolean, modifier: Modifier = Modifier) {
    val colors = KoanTheme.colors
    Box(
        modifier
            .size(18.dp)
            .background(if (picked) colors.accent else colors.bg, RectangleShape)
            .border(KoanShape.hairline, if (picked) colors.accent else colors.muted, RectangleShape),
        contentAlignment = Alignment.Center,
    ) {
        if (picked) KoanIcon("check", tone = KoanTone.Ink, textSizeDp = 12f)
    }
}

/** What the page says when there is nothing to list. */
@Composable
fun ShelfEmpty(icon: String, title: String, detail: String? = null, modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxSize().padding(KoanSpace.page),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        KoanIcon(icon, tone = KoanTone.Muted, textSizeDp = 28f)
        KoanText(title, KoanRole.Body, KoanTone.Strong)
        if (detail != null) KoanText(detail, KoanRole.Meta, KoanTone.Muted, maxLines = 4)
    }
}

/** A page's title with the shelf's counts under it. */
@Composable
fun ShelfHeader(title: String, counts: String) {
    Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        KoanText(title, KoanRole.Title, KoanTone.Strong)
        if (counts.isNotEmpty()) KoanText(counts, KoanRole.Fine, KoanTone.Muted)
    }
}

/** While picking: how many, and what to do with them. Sits at the foot of the page. */
@Composable
fun ShelfPickBar(pick: Pick, onPlay: () -> Unit, onQueue: () -> Unit, onAll: () -> Unit, onDone: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth().background(KoanTheme.colors.bg)) {
        KoanRule()
        Row(
            Modifier.fillMaxWidth().padding(horizontal = KoanSpace.l),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
        ) {
            KoanText("${pick.count} selected", KoanRole.Meta, KoanTone.Muted, Modifier.weight(1f))
            KoanButton("all", onAll, kind = KoanButtonKind.Text)
            KoanButton("queue", onQueue, kind = KoanButtonKind.Bordered, enabled = pick.count > 0)
            KoanButton("play", onPlay, kind = KoanButtonKind.Prominent, enabled = pick.count > 0)
            KoanButton("done", onDone, kind = KoanButtonKind.Text)
        }
    }
}

/** `m:ss`, or `h:mm:ss` from an hour. Nothing for a track whose length is unknown. */
fun shelfDuration(ms: Long?): String {
    if (ms == null || ms < 0) return ""
    val total = ms / 1000
    val h = total / 3600
    val m = (total % 3600) / 60
    val s = total % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%d:%02d".format(m, s)
}

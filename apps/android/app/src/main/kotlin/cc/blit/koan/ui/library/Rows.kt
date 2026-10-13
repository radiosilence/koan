package cc.blit.koan.ui.library

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import cc.blit.koan.art.Art
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.Artist
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanBadge
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.theme.koanRow
import coil3.compose.AsyncImage

/** `3:07`, or `1:02:45` past an hour. */
fun duration(ms: Long): String {
    val s = ms / 1000
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

/** `42 min`, or `1 h 12 min`: a record's length. Under a minute, as a track's. */
fun length(ms: Long): String {
    val m = (ms + 30_000) / 60_000
    return when {
        m == 0L -> duration(ms)
        m >= 60 -> "${m / 60} h ${m % 60} min"
        else -> "$m min"
    }
}

fun count(n: Long, noun: String) = "$n ${if (n == 1L) noun else "${noun}s"}"

/**
 * The bar over a page: back, the page's name with a line under it, and the
 * page's actions. `appText` for a name the app writes, which the theme
 * lowercases; a record's or a person's keeps its case.
 */
@Composable
fun PageBar(
    title: String,
    subtitle: String? = null,
    back: Boolean = true,
    appText: Boolean = true,
    actions: @Composable RowScope.() -> Unit = {},
) {
    val nav = LocalNavigator.current
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .padding(start = if (back) 0.dp else LocalKoanTokens.current.gutter, end = KoanSpace.s),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (back) KoanButton("Back", { nav.pop() }, kind = KoanButtonKind.Icon, icon = KoanIcons.back)
        Column(Modifier.weight(1f)) {
            KoanText(title, role = KoanType.TitleSmall, appText = appText, maxLines = 1)
            if (!subtitle.isNullOrEmpty()) KoanText(subtitle, role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 1)
        }
        actions()
    }
}

/** A cover, square, on the `rule` ground until it arrives; at the list's sleeve size unless the modifier says otherwise. */
@Composable
fun Sleeve(art: Art?, modifier: Modifier = Modifier, size: Dp = LocalKoanTokens.current.thumb) {
    val ground = modifier.size(size).background(KoanTheme.color(KoanTone.Rule))
    if (art == null) {
        Box(ground)
    } else {
        AsyncImage(art, null, ground, contentScale = ContentScale.Crop)
    }
}

/**
 * A tap that goes where `open` says, or ticks while picking; a long press for
 * the menu. No ripple, as the theme has none.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Modifier.tap(playable: Playable, selection: Selection<Playable>?, onMenu: () -> Unit, open: () -> Unit) =
    combinedClickable(
        interactionSource = remember { MutableInteractionSource() },
        indication = null,
        onClick = { if (selection?.take(playable) != true) open() },
        onLongClick = { if (selection?.active != true) onMenu() },
    )

/**
 * One record in a grid: the sleeve with its codec, the title, and the artist
 * and year. A tap opens it, or ticks it while picking; a long press offers
 * the rest.
 */
@Composable
fun AlbumTile(album: Album, modifier: Modifier = Modifier, selection: Selection<Playable>? = null, showArtist: Boolean = true) {
    val nav = LocalNavigator.current
    val playable = Playable.OfAlbum(album)
    val picked = selection?.contains(playable) == true
    var menu by remember { mutableStateOf(false) }
    Column(
        modifier
            .tap(playable, selection, { menu = true }) { nav.push(AlbumRoute(album.id)) }
            .padding(KoanSpace.xs),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.xs),
    ) {
        Box(Modifier.fillMaxWidth().aspectRatio(1f)) {
            Sleeve(Art.album(album.id), Modifier.matchParentSize())
            album.codec?.let { KoanBadge(it.uppercase(), Modifier.align(Alignment.TopEnd).padding(KoanSpace.s)) }
            album.onDevice?.let { on ->
                if (on.total > 0u) DownloadedBar(on.have.toFloat() / on.total.toFloat(), Modifier.align(Alignment.BottomStart))
            }
            if (selection?.active == true) {
                if (picked) Box(Modifier.matchParentSize().border(3.dp, KoanTheme.color(KoanTone.Accent)))
                Tick(picked, Modifier.align(Alignment.TopStart).padding(KoanSpace.s))
            } else {
                AlbumHeart(album.id, Modifier.align(Alignment.BottomEnd), showing = false)
            }
            PlayableMenu(playable, menu) { menu = false }
        }
        KoanText(album.title, role = KoanType.Control, maxLines = 1)
        KoanText(
            listOfNotNull(album.artistName.takeIf { showArtist }, album.year?.toString()).joinToString(" · "),
            role = KoanType.Fine,
            tone = KoanTone.Muted,
            maxLines = 1,
        )
    }
}

/** How much of a record is on this device, along the foot of its sleeve. */
@Composable
private fun DownloadedBar(fraction: Float, modifier: Modifier = Modifier) {
    Box(
        modifier
            .padding(start = KoanSpace.s, end = 48.dp, bottom = KoanSpace.s)
            .fillMaxWidth()
            .height(3.dp)
            .background(KoanTheme.colors.bg.copy(alpha = 0.6f)),
    ) {
        Box(Modifier.fillMaxWidth(fraction.coerceIn(0.05f, 1f)).fillMaxHeight().background(KoanTheme.look.accent))
    }
}

/** A row of a list: a lead, a title over a line of detail, and what goes at its end. */
@Composable
private fun LibraryRow(
    playable: Playable,
    selection: Selection<Playable>?,
    onOpen: () -> Unit,
    modifier: Modifier = Modifier,
    marked: Boolean = false,
    lead: @Composable () -> Unit = {},
    trailing: @Composable RowScope.() -> Unit = {},
    content: @Composable () -> Unit,
) {
    val picked = selection?.contains(playable) == true
    var menu by remember { mutableStateOf(false) }
    Box(modifier) {
        Row(
            Modifier
                .fillMaxWidth()
                .koanRow(picked || marked)
                .tap(playable, selection, { menu = true }, onOpen)
                .padding(start = LocalKoanTokens.current.gutter, end = KoanSpace.xs)
                // The heart's touch target, so a favourite's row is no taller than the rest.
                .heightIn(min = 56.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            if (selection?.active == true) Tick(picked)
            lead()
            Box(Modifier.weight(1f)) { content() }
            if (selection?.active != true) trailing()
        }
        Box(Modifier.align(Alignment.TopEnd)) { PlayableMenu(playable, menu) { menu = false } }
    }
}

@Composable
private fun TitleAndDetail(title: String, detail: String?, tone: KoanTone = KoanTone.Ink) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        KoanText(title, tone = tone, maxLines = 1)
        if (detail != null) KoanText(detail, role = KoanType.Meta, tone = KoanTone.Muted, maxLines = 1)
    }
}

/** A record as a row, for the browser's list layout. */
@Composable
fun AlbumRow(album: Album, modifier: Modifier = Modifier, selection: Selection<Playable>? = null) {
    val nav = LocalNavigator.current
    LibraryRow(
        Playable.OfAlbum(album),
        selection,
        { nav.push(AlbumRoute(album.id)) },
        modifier,
        lead = { Sleeve(Art.album(album.id)) },
        trailing = { AlbumHeart(album.id, showing = false) },
    ) {
        TitleAndDetail(album.title, listOfNotNull(album.artistName, album.year?.toString(), album.codec?.uppercase()).joinToString(" · "))
    }
}

@Composable
fun ArtistRow(artist: Artist, modifier: Modifier = Modifier, selection: Selection<Playable>? = null) {
    val nav = LocalNavigator.current
    LibraryRow(
        Playable.OfArtist(artist.id, artist.name),
        selection,
        { nav.push(ArtistRoute(artist.id)) },
        modifier,
        trailing = { ArtistHeart(artist.id, showing = false) },
    ) {
        TitleAndDetail(
            artist.name,
            listOfNotNull(
                count(artist.albumCount, "album").takeIf { artist.albumCount > 0 },
                count(artist.trackCount, "track"),
            ).joinToString(" · "),
        )
    }
}

/**
 * One track. Its number, or bars while it is the one playing; the title, and
 * the artist where it is not the record's. Gathered from all over
 * (`mixed`), a row carries its own sleeve and names its record.
 */
@Composable
fun TrackRow(
    track: Track,
    lead: String,
    current: Boolean,
    playing: Boolean,
    onPlay: () -> Unit,
    modifier: Modifier = Modifier,
    selection: Selection<Playable>? = null,
    mixed: Boolean = false,
    marked: Boolean = false,
) {
    LibraryRow(
        Playable.OfTrack(track),
        selection,
        onPlay,
        modifier,
        marked,
        lead = {
            if (mixed) {
                Box(contentAlignment = Alignment.Center) {
                    Sleeve(track.albumId?.let { Art.album(it) } ?: Art.track(track.id))
                    if (current) {
                        Box(Modifier.size(LocalKoanTokens.current.thumb).background(KoanTheme.colors.bg.copy(alpha = 0.55f)))
                        PlayingBars(playing)
                    }
                }
            } else {
                Box(Modifier.width(28.dp), contentAlignment = Alignment.CenterStart) {
                    if (current) PlayingBars(playing) else KoanText(lead, role = KoanType.Meta, tone = KoanTone.Muted)
                }
            }
        },
        trailing = {
            TrackHeart(track.id, showing = false)
            track.durationMs?.let { KoanText(duration(it), role = KoanType.Fine, tone = KoanTone.Muted) }
        },
    ) {
        TitleAndDetail(
            track.title,
            if (mixed) "${track.artistName} · ${track.albumTitle}" else track.artistName.takeIf { it != track.albumArtistName },
            tone = if (current) KoanTone.Accent else KoanTone.Ink,
        )
    }
}

/** A heading in a page of sections, with how many there are, opening the section's whole listing. */
@Composable
fun SectionHead(title: String, total: Long, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    KoanButton(onOpen, modifier.padding(horizontal = LocalKoanTokens.current.gutter), kind = KoanButtonKind.Standard) {
        KoanText(title, role = KoanType.Control, appText = true)
        KoanText("$total", Modifier.padding(start = KoanSpace.s), role = KoanType.Control, tone = KoanTone.Muted)
        KoanIcon(KoanIcons.disclosure, "See all", Modifier.padding(start = KoanSpace.xs))
    }
}

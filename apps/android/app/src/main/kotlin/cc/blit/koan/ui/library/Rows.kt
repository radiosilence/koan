package cc.blit.koan.ui.library

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
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
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import cc.blit.koan.R
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
import coil3.compose.AsyncImagePainter

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

/** A cover, square, at the list's sleeve size unless the modifier says otherwise. */
@Composable
fun Sleeve(art: Art?, modifier: Modifier = Modifier, size: Dp = LocalKoanTokens.current.thumb) {
    Cover(art, modifier.size(size))
}

/**
 * A cover filling `modifier`, on the `rule` ground until it arrives, and the
 * kōan ensō on that ground where there is none: nothing playing, or a record
 * without artwork. An artist's photograph that fails stays bare ground, as the
 * ensō stands for a record. `alpha` fades the art and the ensō, not the ground.
 */
@Composable
fun Cover(art: Art?, modifier: Modifier = Modifier, description: String? = null, alpha: Float = 1f) {
    var missing by remember(art) { mutableStateOf(art == null) }
    Box(modifier.background(KoanTheme.color(KoanTone.Rule))) {
        if (missing && art?.kind != Art.Kind.Artist) {
            Image(
                painterResource(R.drawable.koan_enso),
                null,
                Modifier.matchParentSize(),
                alpha = alpha,
                colorFilter = ColorFilter.tint(KoanTheme.color(KoanTone.Muted).copy(alpha = 0.5f)),
            )
        }
        if (art != null) {
            AsyncImage(
                art,
                description,
                Modifier.matchParentSize(),
                contentScale = ContentScale.Crop,
                alpha = alpha,
                onState = { missing = it is AsyncImagePainter.State.Error },
            )
        }
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
 * and year. A tap on the sleeve plays it, a tap below opens it, either ticks
 * it while picking; a long press offers the rest.
 */
@Composable
fun AlbumTile(
    album: Album,
    modifier: Modifier = Modifier,
    selection: Selection<Playable>? = null,
    showArtist: Boolean = true,
    /** The artist whose page this tile is on, whose name it leaves as text. */
    onArtistPage: Long? = null,
) {
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
        // The sleeve plays the record and opens it, as the Apple apps' does;
        // the words under it open it. While picking, either ticks it.
        Box(Modifier.fillMaxWidth().aspectRatio(1f).tap(playable, selection, { menu = true }, rememberPlayNow(playable))) {
            Sleeve(Art.album(album.id), Modifier.matchParentSize())
            album.codec?.let { KoanBadge(it.uppercase(), Modifier.align(Alignment.TopEnd).padding(KoanSpace.s)) }
            album.onDevice?.let { on ->
                if (on.total > 0u) DownloadedBar(on.have.toFloat() / on.total.toFloat(), Modifier.align(Alignment.BottomStart))
            }
            if (selection?.active == true) {
                if (picked) Box(Modifier.matchParentSize().border(3.dp, KoanTheme.color(KoanTone.Accent)))
                Tick(picked, Modifier.align(Alignment.TopStart).padding(KoanSpace.s))
            } else {
                AlbumHeart(album.id, Modifier.align(Alignment.BottomEnd).onGround(), showing = false)
            }
            PlayableMenu(playable, menu) { menu = false }
        }
        KoanText(album.title, role = KoanType.Control, maxLines = 1)
        NameLine(
            listOfNotNull(artistCredit(album.artistCredit, album.artistName, album.artistId, except = onArtistPage).takeIf { showArtist }, album.year?.let { Named("$it") }),
            role = KoanType.Fine,
            linked = selection?.active != true,
        )
    }
}

/**
 * A small square of `bg` behind a heart on a sleeve, so it reads over any
 * cover; a disc in the platform's look.
 */
@Composable
private fun Modifier.onGround(): Modifier {
    val ground = KoanTheme.colors.bg
    val shape = KoanTheme.metric(RectangleShape, CircleShape)
    return drawBehind {
        val side = 28.dp.toPx()
        val outline = shape.createOutline(Size(side, side), layoutDirection, this)
        translate((size.width - side) / 2, (size.height - side) / 2) { drawOutline(outline, ground) }
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
                .padding(horizontal = LocalKoanTokens.current.gutter)
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
private fun TitleAndDetail(title: String, detail: List<Named>, linked: Boolean, tone: KoanTone = KoanTone.Ink) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        KoanText(title, tone = tone, maxLines = 1)
        if (detail.any { it.text.isNotEmpty() }) NameLine(detail, linked = linked)
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
        TitleAndDetail(
            album.title,
            listOfNotNull(artistCredit(album.artistCredit, album.artistName, album.artistId), album.year?.let { Named("$it") }, album.codec?.let { Named(it.uppercase()) }),
            linked = selection?.active != true,
        )
    }
}

@Composable
fun ArtistRow(artist: Artist, modifier: Modifier = Modifier, selection: Selection<Playable>? = null) {
    val nav = LocalNavigator.current
    val playable = Playable.OfArtist(artist.id, artist.name)
    val play = rememberPlayNow(playable)
    LibraryRow(
        playable,
        selection,
        { nav.push(ArtistRoute(artist.id)) },
        modifier,
        // No hover on a phone, so the play button the Mac shows on hover is
        // always there; a tap on the rest of the row opens the artist.
        lead = { if (selection?.active != true) KoanButton("Play ${artist.name}", play, kind = KoanButtonKind.Icon, icon = KoanIcons.play) },
        trailing = { ArtistHeart(artist.id, showing = false) },
    ) {
        TitleAndDetail(
            artist.name,
            listOfNotNull(
                count(artist.albumCount, "album").takeIf { artist.albumCount > 0 },
                count(artist.trackCount, "track"),
            ).map { Named(it) },
            linked = false,
        )
    }
}

/**
 * One track. Its number, or bars while it is the one playing; the title, and
 * the artist where it is not the record's. Gathered from all over
 * (`mixed`), a row carries its own sleeve and names its record. Names are
 * links to their pages, but for the page the row is on.
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
    /** The artist whose page this row is on, whose name it leaves as text. */
    onArtistPage: Long? = null,
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
            TrackAvailability(track)
            TrackHeart(track.id, showing = false)
            track.durationMs?.let { KoanText(duration(it), role = KoanType.Fine, tone = KoanTone.Muted) }
        },
    ) {
        TitleAndDetail(
            track.title,
            listOfNotNull(
                artistCredit(track.artistCredit, track.artistName, track.artistId, except = onArtistPage)
                    .takeIf { mixed || track.artistName != track.albumArtistName },
                albumName(track.albumTitle, track.albumId, track.id).takeIf { mixed },
            ),
            linked = selection?.active != true,
            tone = if (current) KoanTone.Accent else KoanTone.Ink,
        )
    }
}

/** A heading in a page of sections, with how many there are, opening the section's whole listing. */
@Composable
fun SectionHead(title: String, total: Long, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    KoanButton(onOpen, modifier.wrapContentWidth(Alignment.Start), kind = KoanButtonKind.Standard) {
        KoanText(title, role = KoanType.Control, appText = true)
        KoanText("$total", Modifier.padding(start = KoanSpace.s), role = KoanType.Control, tone = KoanTone.Muted)
        KoanIcon(KoanIcons.disclosure, "See all", Modifier.padding(start = KoanSpace.xs))
    }
}

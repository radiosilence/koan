package cc.blit.koan.ui.search

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import cc.blit.koan.KoanApp
import cc.blit.koan.art.Art
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.library.ArtistRoute
import cc.blit.koan.ui.library.NameLine
import cc.blit.koan.ui.library.Named
import cc.blit.koan.ui.library.Playable
import cc.blit.koan.ui.library.PlayableMenu
import cc.blit.koan.ui.library.Selection
import cc.blit.koan.ui.library.Sleeve
import cc.blit.koan.ui.library.Tick
import cc.blit.koan.ui.library.TrackAvailability
import cc.blit.koan.ui.library.albumName
import cc.blit.koan.ui.library.artistCredit
import cc.blit.koan.ui.library.pickable
import cc.blit.koan.ui.library.rememberPlayNow
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanEdge
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.koanChip
import cc.blit.koan.ui.theme.koanRow
import cc.blit.koan.ui.theme.koanRule
import cc.blit.koan.ui.theme.koanSurface

/** The narrowest a record tile goes, as the Apple apps' result grid. */
internal val TileMinimum = 140.dp

/** An artist among other results: a chip going to the artist. */
@Composable
internal fun ArtistPill(id: Long, name: String, selection: Selection<Playable>) {
    val nav = LocalNavigator.current
    val playable = Playable.OfArtist(id, name)
    var menu by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .widthIn(max = 260.dp)
                .koanChip()
                .pickable(playable, selection, { menu = true }) { nav.push(ArtistRoute(id)) },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs),
        ) {
            if (selection.active) Tick(selection.contains(playable))
            KoanText(name, role = KoanType.Control, maxLines = 1)
        }
        PlayableMenu(playable, menu) { menu = false }
    }
}

/**
 * A record: the sleeve plays it, the words under it open it, the artist's name
 * goes to the artist. While picking, any of it ticks the record.
 */
@Composable
internal fun AlbumTile(album: Album, selection: Selection<Playable>) {
    val nav = LocalNavigator.current
    val playable = Playable.OfAlbum(album)
    val picked = selection.contains(playable)
    var menu by remember { mutableStateOf(false) }
    val showMenu = { menu = true }
    Column(verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .pickable(playable, selection, showMenu, rememberPlayNow(playable)),
        ) {
            Sleeve(Art.album(album.id), Modifier.matchParentSize())
            if (selection.active) {
                if (picked) Box(Modifier.matchParentSize().border(3.dp, KoanTheme.color(KoanTone.Accent)))
                Tick(picked, Modifier.align(Alignment.TopStart).padding(KoanSpace.s))
            }
            PlayableMenu(playable, menu) { menu = false }
        }
        KoanText(
            album.title,
            Modifier.fillMaxWidth().pickable(playable, selection, showMenu) { nav.push(AlbumRoute(album.id)) },
            role = KoanType.Control,
            maxLines = 1,
        )
        NameLine(
            listOfNotNull(artistCredit(album.artistCredit, album.artistName, album.artistId), album.year?.let { Named("$it") }),
            Modifier.fillMaxWidth().pickable(playable, selection, showMenu) { nav.push(AlbumRoute(album.id)) },
            role = KoanType.Fine,
            linked = !selection.active,
        )
    }
}

/** A track points at its record: that is where it lives, and where it is played from. */
@Composable
internal fun TrackRow(track: Track, selection: Selection<Playable>) {
    val nav = LocalNavigator.current
    val playable = Playable.OfTrack(track)
    val picked = selection.contains(playable)
    var menu by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .fillMaxWidth()
                .koanRow(picked)
                .pickable(playable, selection, { menu = true }) { track.albumId?.let { nav.push(AlbumRoute(it, track.id)) } }
                .padding(vertical = KoanSpace.xs),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            if (selection.active) Tick(picked)
            Sleeve(track.albumId?.let { Art.album(it) } ?: Art.track(track.id), size = 40.dp)
            Column(Modifier.weight(1f)) {
                KoanText(track.title, maxLines = 1)
                NameLine(
                    listOf(artistCredit(track.artistCredit, track.artistName, track.artistId), albumName(track.albumTitle, track.albumId, track.id)),
                    role = KoanType.Fine,
                    linked = !selection.active,
                )
            }
            TrackAvailability(track)
            track.durationMs?.let { KoanText(duration(it), role = KoanType.Fine, tone = KoanTone.Muted) }
        }
        PlayableMenu(playable, menu) { menu = false }
    }
}

private fun duration(ms: Long): String {
    val s = ms / 1000
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

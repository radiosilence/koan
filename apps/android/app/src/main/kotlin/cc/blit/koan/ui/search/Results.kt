package cc.blit.koan.ui.search

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.role
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
import cc.blit.koan.ffi.Album
import cc.blit.koan.ffi.Track
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.library.ArtistRoute
import cc.blit.koan.ui.library.Playable
import cc.blit.koan.ui.library.PlayableMenu
import cc.blit.koan.ui.library.Sleeve
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.playlists.AddToPlaylistSheet
import cc.blit.koan.ui.theme.KoanButton
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

/**
 * A result's taps: inside select mode a tap ticks it, outside it goes where
 * `open` says. A long press opens the library's menu for it, as everywhere
 * else in the app.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Modifier.result(playable: Playable, selection: Selection, onMenu: () -> Unit, open: () -> Unit) =
    combinedClickable(
        interactionSource = remember { MutableInteractionSource() },
        indication = null,
        onClick = { if (!selection.take(playable)) open() },
        onLongClick = { if (!selection.active) onMenu() },
    )

/** A tick, while picking: the empty box or the filled one. */
@Composable
private fun Tick(picked: Boolean, modifier: Modifier = Modifier) {
    KoanIcon(
        if (picked) KoanIcons.picked else KoanIcons.unpicked,
        if (picked) "Selected" else "Not selected",
        modifier,
        color = KoanTheme.color(if (picked) KoanTone.Accent else KoanTone.Muted),
    )
}

/** An artist among other results: a chip going to the artist. */
@Composable
internal fun ArtistPill(id: Long, name: String, selection: Selection) {
    val nav = LocalNavigator.current
    val playable = Playable.OfArtist(id, name)
    var menu by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .widthIn(max = 260.dp)
                .koanChip()
                .result(playable, selection, { menu = true }) { nav.push(ArtistRoute(id)) },
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
 * A record: the sleeve plays it, the title opens it, the artist goes to the
 * artist. While picking, any of it ticks the record.
 */
@Composable
internal fun AlbumTile(album: Album, selection: Selection, onSleeve: () -> Unit) {
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
                .result(playable, selection, showMenu, onSleeve),
        ) {
            Sleeve(album.id, Modifier.matchParentSize(), corner = 0)
            if (selection.active) {
                if (picked) Box(Modifier.matchParentSize().border(3.dp, KoanTheme.color(KoanTone.Accent)))
                Tick(picked, Modifier.align(Alignment.TopStart).padding(KoanSpace.s))
            }
            PlayableMenu(playable, menu) { menu = false }
        }
        KoanText(
            album.title,
            Modifier.fillMaxWidth().result(playable, selection, showMenu) { nav.push(AlbumRoute(album.id)) },
            role = KoanType.Control,
            maxLines = 1,
        )
        KoanText(
            listOfNotNull(album.artistName, album.year?.toString()).joinToString(" · "),
            Modifier.fillMaxWidth().result(playable, selection, showMenu) { nav.push(ArtistRoute(album.artistId)) },
            role = KoanType.Fine,
            tone = KoanTone.Muted,
            maxLines = 1,
        )
    }
}

/** A track points at its record: that is where it lives, and where it is played from. */
@Composable
internal fun TrackRow(track: Track, selection: Selection) {
    val nav = LocalNavigator.current
    val playable = Playable.OfTrack(track)
    val picked = selection.contains(playable)
    var menu by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .fillMaxWidth()
                .koanRow(picked)
                .result(playable, selection, { menu = true }) { track.albumId?.let { nav.push(AlbumRoute(it)) } }
                .padding(vertical = KoanSpace.xs),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            if (selection.active) Tick(picked)
            Sleeve(track.albumId, Modifier.size(40.dp), corner = 0)
            Column(Modifier.weight(1f)) {
                KoanText(track.title, maxLines = 1)
                KoanText("${track.artistName} — ${track.albumTitle}", role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 1)
            }
            track.durationMs?.let { KoanText(duration(it), role = KoanType.Fine, tone = KoanTone.Muted) }
        }
        PlayableMenu(playable, menu) { menu = false }
    }
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
    var adding by remember { mutableStateOf<List<Playable>?>(null) }
    fun run(action: (List<Long>) -> Unit) {
        val items = selection.picked.toList()
        selection.end()
        app.mirror.launch {
            val ids = items.flatMap { it.trackIds(this) }
            if (ids.isNotEmpty()) action(ids)
        }
    }
    Column(
        Modifier
            .fillMaxWidth()
            .koanSurface()
            .koanRule(KoanEdge.Top)
            .padding(horizontal = KoanSpace.l, vertical = KoanSpace.xs),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            KoanText(
                if (count == 0) "Select items" else "$count selected",
                Modifier.weight(1f),
                role = KoanType.Fine,
                tone = KoanTone.Muted,
                appText = true,
            )
            KoanButton("Done", selection::end)
        }
        Row(Modifier.fillMaxWidth()) {
            BarItem("Play", "play", KoanIcons.play, count > 0) { run(app.player::playTracks) }
            BarItem("Play Next", "next", KoanIcons.playNext, count > 0) { run(app.player::playNext) }
            BarItem("Add to Queue", "queue", KoanIcons.queue, count > 0) { run(app.player::enqueue) }
            BarItem("Add to Playlist", "playlist", KoanIcons.playlist, count > 0) { adding = selection.picked.toList() }
            BarItem("Favourite", "favourite", KoanIcons.favourite, count > 0) { favourite(app, selection) }
        }
    }
    adding?.let { items ->
        AddToPlaylistSheet({ items.flatMap { it.trackIds(this) } }, onAdded = selection::end) { adding = null }
    }
}

/**
 * One of the selection bar's verbs: its glyph over a short word, an equal
 * share of the bar each, so five fit a phone. Read out by its full name.
 */
@Composable
private fun RowScope.BarItem(title: String, short: String, icon: Int, enabled: Boolean, onClick: () -> Unit) {
    Column(
        Modifier
            .weight(1f)
            .heightIn(min = 48.dp)
            .alpha(if (enabled) 1f else 0.4f)
            .clickable(enabled = enabled, role = Role.Button, onClickLabel = title, onClick = onClick)
            .clearAndSetSemantics {
                contentDescription = title
                role = Role.Button
                if (!enabled) disabled()
            }
            .padding(vertical = KoanSpace.xs),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(KoanSpace.xs, Alignment.CenterVertically),
    ) {
        if (KoanTheme.icons || !KoanTheme.isOn) KoanIcon(icon, null)
        KoanText(short, role = KoanType.Fine, appText = true, maxLines = 1)
    }
}

/**
 * Hearts every one of the pick, or, where all of them are hearted already,
 * takes the hearts off: the things themselves, not the tracks behind them.
 */
private fun favourite(app: KoanApp, selection: Selection) {
    val items = selection.picked.toList()
    selection.end()
    app.mirror.launch {
        val tracks = favouriteTrackIds().toSet()
        val albums = favouriteAlbumIds().toSet()
        val artists = favouriteArtistIds().toSet()
        fun hearted(item: Playable) = when (item) {
            is Playable.OfTrack -> item.track.id in tracks
            is Playable.OfAlbum -> item.album.id in albums
            is Playable.OfArtist -> item.id in artists
        }
        val all = items.all(::hearted)
        for (item in items) {
            if (hearted(item) != all) continue
            when (item) {
                is Playable.OfTrack -> toggleFavourite(item.track.id)
                is Playable.OfAlbum -> toggleFavouriteAlbum(item.album.id)
                is Playable.OfArtist -> toggleFavouriteArtist(item.id)
            }
        }
    }
}

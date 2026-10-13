package cc.blit.koan.ui.library

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableStateSetOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import cc.blit.koan.ui.koanApp
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
import cc.blit.koan.ui.theme.koanRule
import cc.blit.koan.ui.theme.koanSurface

/** What a playable is, as one value, for telling picks apart. */
val Playable.key
    get() = when (this) {
        is Playable.OfTrack -> "track:${track.id}"
        is Playable.OfAlbum -> "album:${album.id}"
        is Playable.OfArtist -> "artist:$id"
    }

/**
 * Results picked to play or queue together, as `PlayableSelection` is on the
 * Apple apps. A mode: outside it a tap goes somewhere, inside it a tap ticks.
 *
 * Held in the order things were ticked rather than the page's, so a pick can
 * gather from several queries: search, tick, search again, tick more.
 */
class Selection {
    var active by mutableStateOf(false)
        private set

    /** What is ticked, in the order it was ticked. */
    val picked = mutableStateListOf<Playable>()
    private val members = mutableStateSetOf<String>()

    fun contains(playable: Playable) = playable.key in members

    fun begin() {
        active = true
    }

    fun end() {
        active = false
        picked.clear()
        members.clear()
    }

    /** A tap on something that goes somewhere outside the mode. Whether the pick took it. */
    fun take(playable: Playable): Boolean {
        if (!active) return false
        if (members.remove(playable.key)) {
            picked.removeAll { it.key == playable.key }
        } else {
            members += playable.key
            picked += playable
        }
        return true
    }
}

/**
 * A pickable thing's taps: inside select mode a tap ticks it, outside it goes where
 * `open` says. A long press opens the library's menu for it, as everywhere
 * else in the app.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun Modifier.pickable(playable: Playable, selection: Selection, onMenu: () -> Unit, open: () -> Unit) =
    combinedClickable(
        interactionSource = remember { MutableInteractionSource() },
        indication = null,
        onClick = { if (!selection.take(playable)) open() },
        onLongClick = { if (!selection.active) onMenu() },
    )

/** A tick, while picking: the empty box or the filled one. */
@Composable
fun Tick(picked: Boolean, modifier: Modifier = Modifier) {
    KoanIcon(
        if (picked) KoanIcons.picked else KoanIcons.unpicked,
        if (picked) "Selected" else "Not selected",
        modifier,
        color = KoanTheme.color(if (picked) KoanTone.Accent else KoanTone.Muted),
    )
}

/**
 * What to do with the pick, at the foot of the page. Each verb resolves the
 * pick to tracks in the order it was made, acts, and ends the mode.
 */
@Composable
fun SelectionBar(selection: Selection) {
    val app = koanApp()
    val store = favourites()
    val count = selection.picked.size
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
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
            KoanButton("Play", { run(app.player::playTracks) }, kind = KoanButtonKind.Text, icon = KoanIcons.play, enabled = count > 0)
            KoanButton("Next", { run(app.player::playNext) }, kind = KoanButtonKind.Text, icon = KoanIcons.playNext, enabled = count > 0)
            KoanButton("Queue", { run(app.player::enqueue) }, kind = KoanButtonKind.Text, icon = KoanIcons.queue, enabled = count > 0)
            KoanButton("Favourite", { favourite(store, selection) }, kind = KoanButtonKind.Text, icon = KoanIcons.favourite, enabled = count > 0)
        }
    }
}

/**
 * Hearts every one of the pick, or, where all of them are hearted already,
 * takes the hearts off: the things themselves, not the tracks behind them.
 */
private fun favourite(store: Favourites, selection: Selection) {
    val items = selection.picked.toList()
    selection.end()
    fun hearted(item: Playable) = when (item) {
        is Playable.OfTrack -> item.track.id in store.tracks.value
        is Playable.OfAlbum -> item.album.id in store.albums.value
        is Playable.OfArtist -> item.id in store.artists.value
    }
    val all = items.all(::hearted)
    for (item in items) {
        if (hearted(item) != all) continue
        when (item) {
            is Playable.OfTrack -> store.toggleTrack(item.track.id)
            is Playable.OfAlbum -> store.toggleAlbum(item.album.id)
            is Playable.OfArtist -> store.toggleArtist(item.id)
        }
    }
}

/** The bar button that starts and ends picking. */
@Composable
fun SelectButton(selection: Selection) {
    KoanButton({ if (selection.active) selection.end() else selection.begin() }, kind = KoanButtonKind.Icon) {
        KoanIcon(if (selection.active) KoanIcons.selecting else KoanIcons.select, "Select")
    }
}

/**
 * A pick belongs to the page it was made on: it ends when the page goes, and
 * back ends it before it leaves the page.
 */
@Composable
fun SelectMode(selection: Selection) {
    DisposableEffect(selection) { onDispose { selection.end() } }
    // Composed only while picking, so it registers after the shell's own
    // handler for a tab's root and is asked first.
    if (selection.active) BackHandler { selection.end() }
}

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
import cc.blit.koan.ui.playlists.AddToPlaylistSheet
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
 * Things picked to act on together, as `PlayableSelection` is on the Apple
 * apps. A mode: outside it a tap goes somewhere, inside it a tap ticks.
 *
 * Held in the order things were ticked rather than the page's, so a pick can
 * gather from several queries: search, tick, search again, tick more. Told
 * apart by `key`: a playable by what it is, a playlist entry or a play by its
 * own id, since the same track can be two of them.
 */
class Selection<T>(private val key: (T) -> Any) {
    var active by mutableStateOf(false)
        private set

    /** What is ticked, in the order it was ticked. */
    val picked = mutableStateListOf<T>()
    private val members = mutableStateSetOf<Any>()

    fun contains(item: T) = key(item) in members

    fun begin() {
        active = true
    }

    fun end() {
        active = false
        picked.clear()
        members.clear()
    }

    /** A tap on something that goes somewhere outside the mode. Whether the pick took it. */
    fun take(item: T): Boolean {
        if (!active) return false
        val k = key(item)
        if (members.remove(k)) {
            picked.removeAll { key(it) == k }
        } else {
            members += k
            picked += item
        }
        return true
    }
}

/** A pick of artists, records and tracks, told apart by what each is. */
fun playableSelection() = Selection<Playable> { it.key }

/**
 * A pickable thing's taps: inside select mode a tap ticks it, outside it goes where
 * `open` says. A long press opens the library's menu for it, as everywhere
 * else in the app.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun Modifier.pickable(playable: Playable, selection: Selection<Playable>, onMenu: () -> Unit, open: () -> Unit) =
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
fun SelectionBar(selection: Selection<Playable>) = SelectionBar(selection, { it })

/**
 * Taking the pick out of what the page shows: a playlist's entries, or plays
 * from the history. `title` names the verb ("Remove", "Forget").
 */
class Removal<T>(val title: String, val act: (List<T>) -> Unit)

/**
 * The bar over a pick of things that are not playables themselves (a
 * playlist's entries, the history's plays): each stands for one, by
 * `playable`, and `remove`, when given, takes the pick off the page.
 */
@Composable
fun <T> SelectionBar(selection: Selection<T>, playable: (T) -> Playable, remove: Removal<T>? = null) {
    val app = koanApp()
    val store = favourites()
    val count = selection.picked.size
    var adding by remember { mutableStateOf<List<Playable>?>(null) }
    fun run(action: (List<Long>) -> Unit) {
        val items = selection.picked.map(playable)
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
            KoanButton("Playlist", { adding = selection.picked.map(playable) }, kind = KoanButtonKind.Text, icon = KoanIcons.playlist, enabled = count > 0)
            KoanButton("Favourite", { favourite(store, selection.picked.map(playable)); selection.end() }, kind = KoanButtonKind.Text, icon = KoanIcons.favourite, enabled = count > 0)
            remove?.let { removal ->
                KoanButton(
                    removal.title,
                    { removal.act(selection.picked.toList()); selection.end() },
                    kind = KoanButtonKind.Text,
                    icon = KoanIcons.remove,
                    enabled = count > 0,
                    destructive = true,
                )
            }
        }
    }
    adding?.let { items ->
        AddToPlaylistSheet({ items.flatMap { it.trackIds(this) } }, onAdded = selection::end) { adding = null }
    }
}

/**
 * Hearts every one of the pick, or, where all of them are hearted already,
 * takes the hearts off: the things themselves, not the tracks behind them.
 */
private fun favourite(store: Favourites, items: List<Playable>) {
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
fun SelectButton(selection: Selection<*>) {
    KoanButton({ if (selection.active) selection.end() else selection.begin() }, kind = KoanButtonKind.Icon) {
        KoanIcon(if (selection.active) KoanIcons.selecting else KoanIcons.select, "Select")
    }
}

/**
 * A pick belongs to the page it was made on: it ends when the page goes, and
 * back ends it before it leaves the page.
 */
@Composable
fun SelectMode(selection: Selection<*>) {
    DisposableEffect(selection) { onDispose { selection.end() } }
    // Composed only while picking, so it registers after the shell's own
    // handler for a tab's root and is asked first.
    if (selection.active) BackHandler { selection.end() }
}

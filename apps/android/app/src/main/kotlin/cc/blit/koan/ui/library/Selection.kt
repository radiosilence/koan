package cc.blit.koan.ui.library

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.TextAutoSize
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.role
import androidx.compose.ui.unit.dp
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
 * from the history. `title` names the verb ("Remove", "Forget"), `noun` one of
 * the pick ("track", "play") for the question asked first: these pages have
 * no undo, and a bulk removal is a lot to lose to a stray tap.
 */
class Removal<T>(val title: String, val noun: String, val act: (List<T>) -> Unit)

/**
 * The bar over a pick of things that are not playables themselves (a
 * playlist's entries, the history's plays): each stands for one, by
 * `playable`, and `remove`, when given, takes the pick off the page.
 */
@Composable
fun <T> SelectionBar(selection: Selection<T>, playable: (T) -> Playable, remove: Removal<T>? = null) {
    val app = koanApp()
    val store = favourites()
    val picks = selection.picked.size
    var adding by remember { mutableStateOf<List<Playable>?>(null) }
    fun run(action: (List<Long>) -> Unit) {
        val items = selection.picked.map(playable)
        selection.end()
        app.mirror.launch {
            val ids = items.flatMap { it.trackIds(this) }
            if (ids.isNotEmpty()) action(ids)
        }
    }
    var confirming by remember { mutableStateOf<List<T>?>(null) }
    SelectionBarFrame(picks, selection::end) {
        BarItem("Play", "play", KoanIcons.play, picks > 0) { run(app.player::playTracks) }
        BarItem("Play Next", "next", KoanIcons.playNext, picks > 0) { run(app.player::playNext) }
        BarItem("Add to Queue", "queue", KoanIcons.queue, picks > 0) { run(app.player::enqueue) }
        BarItem("Add to Playlist", "playlist", KoanIcons.playlist, picks > 0) { adding = selection.picked.map(playable) }
        BarItem("Favourite", "favourite", KoanIcons.favourite, picks > 0) { favourite(store, selection.picked.map(playable)); selection.end() }
        remove?.let { removal ->
            BarItem(removal.title, removal.title, KoanIcons.remove, picks > 0, destructive = true) { confirming = selection.picked.toList() }
        }
    }
    if (remove != null) {
        confirming?.let { picked ->
            AlertDialog(
                onDismissRequest = { confirming = null },
                title = { KoanText("${remove.title} ${count(picked.size.toLong(), remove.noun)}?", role = KoanType.TitleSmall) },
                confirmButton = {
                    KoanButton(remove.title, {
                        confirming = null
                        remove.act(picked)
                        selection.end()
                    }, destructive = true)
                },
                dismissButton = { KoanButton("Cancel", { confirming = null }) },
            )
        }
    }
    adding?.let { items ->
        AddToPlaylistSheet({ items.flatMap { it.trackIds(this) } }, onAdded = selection::end) { adding = null }
    }
}

/**
 * The selection bar's frame, at the foot of the page: how many are picked and
 * Done over a row of `BarItem`s.
 */
@Composable
internal fun SelectionBarFrame(count: Int, done: () -> Unit, verbs: @Composable RowScope.() -> Unit) {
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
            KoanButton("Done", done)
        }
        Row(Modifier.fillMaxWidth(), content = verbs)
    }
}

/**
 * One of the selection bar's verbs: its glyph over a short word, an equal
 * share of the bar each, so five fit a phone. Read out by its full name.
 */
@Composable
internal fun RowScope.BarItem(title: String, short: String, icon: Int, enabled: Boolean, destructive: Boolean = false, onClick: () -> Unit) {
    val tone = if (destructive) KoanTone.Bad else KoanTone.Ink
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
        if (KoanTheme.icons || !KoanTheme.isOn) KoanIcon(icon, null, color = KoanTheme.color(tone))
        // Shrinks to three quarters before it would run into its neighbour, as the Swift bar's does.
        val style = KoanType.Fine.style.copy(color = KoanTheme.textColor(tone))
        BasicText(
            KoanTheme.label(short),
            Modifier.padding(horizontal = 2.dp),
            style = style,
            maxLines = 1,
            autoSize = TextAutoSize.StepBased(minFontSize = style.fontSize * 0.75f, maxFontSize = style.fontSize),
        )
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

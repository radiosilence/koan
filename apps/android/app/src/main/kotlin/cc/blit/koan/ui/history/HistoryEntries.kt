package cc.blit.koan.ui.history

import androidx.compose.ui.unit.dp
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.art.Art
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.PlayHistoryEntry
import cc.blit.koan.ui.downloads.DownloadMenuItems
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.AlbumRoute
import cc.blit.koan.ui.navigation.LocalNavigator
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.nowplaying.clock
import cc.blit.koan.ui.playlists.AddToPlaylistSheet
import cc.blit.koan.ui.playlists.CollectionRow
import cc.blit.koan.ui.playlists.ConfirmDialog
import cc.blit.koan.ui.playlists.MenuItem
import cc.blit.koan.ui.playlists.PageBar
import cc.blit.koan.ui.playlists.Sleeve
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.theme.koanSurface
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.stateIn
import kotlinx.serialization.Serializable

@Serializable
data class HistoryRoute(val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.historyEntries() {
    entry<HistoryRoute> { HistoryPage() }
}

/**
 * Every play, most recent first. Asked for again when a play is recorded or
 * forgotten (`history`), and when the library moves, since an entry's track
 * can leave it.
 */
class HistoryModel(mirror: EngineMirror) : ViewModel() {
    val entries: StateFlow<List<PlayHistoryEntry>?> = combine(mirror.history, mirror.library) { h, l -> h to l }
        .mapLatest { runCatching { mirror.engine().playHistory(null) }.getOrDefault(emptyList()) }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)
}

/**
 * What has been listened to, grouped by day, which is how people reach for it
 * ("what was that on Tuesday"). A list of plays, not of tracks: a record played
 * three times is three runs of rows. Swipe a play away, or pick Remove from
 * History, to forget it; the track itself is untouched.
 */
@Composable
private fun HistoryPage() {
    val app = koanApp()
    val model = viewModel { HistoryModel(app.mirror) }
    val entries by model.entries.collectAsStateWithLifecycle()
    var clearing by remember { mutableStateOf(false) }
    var adding by remember { mutableStateOf<Long?>(null) }
    val zone = ZoneId.systemDefault()
    val today = LocalDate.now(zone)
    val gutter = LocalKoanTokens.current.gutter

    Column {
        PageBar("History") {
            KoanButton("Clear…", { clearing = true }, kind = KoanButtonKind.Text, enabled = !entries.isNullOrEmpty())
        }
        val shown = entries
        when {
            shown == null -> Unit
            shown.isEmpty() -> KoanUnavailable("Nothing played yet", KoanIcons.history, "Everything you play lands here, most recent first.")
            else -> LazyColumn {
                byDay(shown, zone) { it.playedAt }.forEach { (day, plays) ->
                    stickyHeader(key = "day:$day") {
                        KoanSectionHeader(
                            dayLabel(day, today),
                            Modifier.fillMaxWidth().koanSurface().padding(horizontal = gutter).padding(bottom = 4.dp),
                        )
                    }
                    items(plays, key = { it.id }) { entry ->
                        HistoryRow(entry, zone, onAdd = { adding = it }, modifier = Modifier.animateItem())
                    }
                }
            }
        }
    }

    adding?.let { id -> AddToPlaylistSheet({ listOf(id) }) { adding = null } }
    if (clearing) {
        ConfirmDialog(
            "Clear History?",
            "Every play is forgotten, here and on every device signed in to your account. This cannot be undone.",
            "Clear",
            onDismiss = { clearing = false },
        ) { app.mirror.launch { clearPlayHistory() } }
    }
}

@Composable
private fun HistoryRow(entry: PlayHistoryEntry, zone: ZoneId, onAdd: (Long) -> Unit, modifier: Modifier) {
    val app = koanApp()
    val nav = LocalNavigator.current
    val track = entry.track
    val forget = { app.mirror.launch { deletePlays(listOf(entry.id)) } }
    val swipe = rememberSwipeToDismissBoxState()
    var menu by remember { mutableStateOf(false) }
    // A play another client scrobbled in did not happen here, and saying so stops it reading as a phantom.
    val elsewhere = if (entry.source != "local") " · scrobbled by another client" else ""

    SwipeToDismissBox(
        state = swipe,
        modifier = modifier,
        enableDismissFromStartToEnd = false,
        onDismiss = { forget() },
        backgroundContent = {
            Box(
                Modifier.fillMaxSize().koanSurface().padding(horizontal = LocalKoanTokens.current.gutter),
                contentAlignment = Alignment.CenterEnd,
            ) {
                KoanIcon(KoanIcons.clear, "Forget", color = KoanTheme.color(KoanTone.Bad))
            }
        },
    ) {
        Box(Modifier.koanSurface()) {
            CollectionRow(
                track.title,
                "${timeOf(entry.playedAt, zone)} · ${track.artistName} · ${track.albumTitle}$elsewhere",
                onClick = { app.player.playTracks(listOf(track.id)) },
                onLongClick = { menu = true },
                leading = { Sleeve(track.albumId?.let { Art.album(it) } ?: Art.track(track.id)) },
                trailing = { KoanText(track.durationMs?.let(::clock).orEmpty(), role = KoanType.Meta, tone = KoanTone.Muted) },
            )
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                MenuItem("Play", KoanIcons.play) { menu = false; app.player.playTracks(listOf(track.id)) }
                MenuItem("Play Next", KoanIcons.playNext) { menu = false; app.player.playNext(listOf(track.id)) }
                MenuItem("Add to Queue", KoanIcons.queue) { menu = false; app.player.enqueue(listOf(track.id)) }
                MenuItem("Add to Playlist…", KoanIcons.playlist) { menu = false; onAdd(track.id) }
                DownloadMenuItems(onDisk = track.onDisk, dismiss = { menu = false }) { listOf(track.id) }
                track.albumId?.let { albumId ->
                    MenuItem("Go to Album", KoanIcons.album) { menu = false; nav.push(AlbumRoute(albumId)) }
                }
                MenuItem("Remove from History", KoanIcons.remove, destructive = true) { menu = false; forget() }
            }
        }
    }
}

/**
 * Runs of consecutive items that fall on the same local day. The history is
 * already in time order, so one pass is enough, and a day cannot be split.
 */
fun <T> byDay(items: List<T>, zone: ZoneId, epochSeconds: (T) -> Long): List<Pair<LocalDate, List<T>>> {
    val out = mutableListOf<Pair<LocalDate, MutableList<T>>>()
    for (item in items) {
        val day = Instant.ofEpochSecond(epochSeconds(item)).atZone(zone).toLocalDate()
        val last = out.lastOrNull()
        if (last?.first == day) last.second.add(item) else out.add(day to mutableListOf(item))
    }
    return out
}

private val dayFormat = DateTimeFormatter.ofLocalizedDate(FormatStyle.FULL)
private val timeFormat = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT)

/** "Today", "Yesterday", or the date in full. */
fun dayLabel(day: LocalDate, today: LocalDate): String = when (day) {
    today -> "Today"
    today.minusDays(1) -> "Yesterday"
    else -> day.format(dayFormat)
}

private fun timeOf(epochSeconds: Long, zone: ZoneId) = Instant.ofEpochSecond(epochSeconds).atZone(zone).format(timeFormat)

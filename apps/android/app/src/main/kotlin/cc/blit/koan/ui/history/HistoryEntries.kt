@file:OptIn(ExperimentalCoroutinesApi::class)

package cc.blit.koan.ui.history

import android.text.format.DateFormat
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.art.Art
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.PlayHistoryEntry
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.library.FilterField
import cc.blit.koan.ui.library.Named
import cc.blit.koan.ui.library.PageBar
import cc.blit.koan.ui.library.Playable
import cc.blit.koan.ui.library.PlayableMenu
import cc.blit.koan.ui.library.Removal
import cc.blit.koan.ui.library.SelectButton
import cc.blit.koan.ui.library.SelectMode
import cc.blit.koan.ui.library.Selection
import cc.blit.koan.ui.library.SelectionBar
import cc.blit.koan.ui.library.Sleeve
import cc.blit.koan.ui.library.Tick
import cc.blit.koan.ui.library.albumName
import cc.blit.koan.ui.library.artistCredit
import cc.blit.koan.ui.library.count
import cc.blit.koan.ui.navigation.nonce
import cc.blit.koan.ui.nowplaying.clock
import cc.blit.koan.ui.playlists.CollectionRow
import cc.blit.koan.ui.playlists.ConfirmDialog
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanDivider
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.theme.koanSurface
import java.text.DecimalFormatSymbols
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Calendar
import java.util.Date
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
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
 * Every play, most recent first, narrowed by `query`. Asked for again when a
 * play is recorded or forgotten (`history`), when the library moves, since an
 * entry's track can leave it, and as the query is typed.
 */
class HistoryModel(mirror: EngineMirror) : ViewModel() {
    val query = MutableStateFlow("")

    val entries: StateFlow<List<PlayHistoryEntry>?> = combine(mirror.history, mirror.library, query) { _, _, q -> q.trim() }
        .mapLatest { q -> runCatching { mirror.engine().playHistory(q.ifEmpty { null }) }.getOrDefault(emptyList()) }
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
    val query by model.query.collectAsStateWithLifecycle()
    var clearing by remember { mutableStateOf(false) }
    val zone = ZoneId.systemDefault()
    val today = LocalDate.now(zone)
    val gutter = LocalKoanTokens.current.gutter
    val times = rememberPlayClock()
    // Plays, not tracks: a track played twice is two rows to pick.
    val selection = remember { Selection<PlayHistoryEntry> { it.id } }
    SelectMode(selection)

    Column {
        PageBar("History", entries?.let { count(it.size.toLong(), "play") }) {
            if (!entries.isNullOrEmpty()) SelectButton(selection)
            KoanButton("Clear…", { clearing = true }, kind = KoanButtonKind.Text, enabled = !entries.isNullOrEmpty() && query.isBlank())
        }
        FilterField(query, "Filter history", { model.query.value = it })
        val shown = entries
        when {
            shown == null -> Unit
            shown.isEmpty() && query.isNotBlank() -> KoanUnavailable("No matches", KoanIcons.search, null)
            shown.isEmpty() -> KoanUnavailable("Nothing played yet", KoanIcons.history, "Everything you play lands here, most recent first.")
            else -> LazyColumn(Modifier.weight(1f)) {
                byDay(shown, zone) { it.playedAt }.forEach { (day, plays) ->
                    stickyHeader(key = "day:$day") {
                        KoanSectionHeader(
                            dayLabel(day, today),
                            Modifier.fillMaxWidth().koanSurface().padding(horizontal = gutter).padding(bottom = 4.dp),
                        )
                    }
                    items(plays, key = { it.id }) { entry ->
                        HistoryRow(entry, times, selection, modifier = Modifier.animateItem())
                    }
                }
            }
        }
        if (selection.active) {
            SelectionBar(
                selection,
                { Playable.OfTrack(it.track) },
                Removal("Forget", "play") { picked -> app.mirror.launch { deletePlays(picked.map { it.id }) } },
            )
        }
    }

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
private fun HistoryRow(entry: PlayHistoryEntry, times: PlayClock, selection: Selection<PlayHistoryEntry>, modifier: Modifier) {
    val app = koanApp()
    val track = entry.track
    val forget = { app.mirror.launch { deletePlays(listOf(entry.id)) } }
    val swipe = rememberSwipeToDismissBoxState()
    var menu by remember { mutableStateOf(false) }
    // A play another client scrobbled in did not happen here, and saying so stops it reading as a phantom.
    val elsewhere = if (entry.source != "local") "scrobbled by another client" else ""

    SwipeToDismissBox(
        state = swipe,
        modifier = modifier,
        enableDismissFromStartToEnd = false,
        enableDismissFromEndToStart = !selection.active,
        onDismiss = { forget() },
        backgroundContent = {
            if (swipe.dismissDirection == SwipeToDismissBoxValue.Settled) return@SwipeToDismissBox
            Box(
                Modifier.fillMaxSize().koanSurface().padding(horizontal = LocalKoanTokens.current.gutter),
                contentAlignment = Alignment.CenterEnd,
            ) {
                KoanIcon(KoanIcons.clear, "Forget", color = KoanTheme.color(KoanTone.Bad))
            }
        },
    ) {
        // On the wash, but for while a swipe uncovers what is behind the row.
        Box(if (swipe.dismissDirection == SwipeToDismissBoxValue.Settled) Modifier else Modifier.koanSurface()) {
            CollectionRow(
                track.title,
                null,
                names = listOf(
                    artistCredit(track.artistCredit, track.artistName, track.artistId),
                    albumName(track.albumTitle, track.albumId, track.id),
                    Named(elsewhere),
                ),
                linked = !selection.active,
                onClick = { if (!selection.take(entry)) app.player.playTracks(listOf(track.id)) },
                onLongClick = if (selection.active) null else ({ menu = true }),
                leading = {
                    if (selection.active) Tick(selection.contains(entry))
                    times.Lead(entry.playedAt)
                    Sleeve(track.albumId?.let { Art.album(it) } ?: Art.track(track.id))
                },
                trailing = { KoanText(track.durationMs?.let(::clock).orEmpty(), role = KoanType.Meta, tone = KoanTone.Muted) },
            )
            PlayableMenu(Playable.OfTrack(track), menu, more = {
                KoanDivider()
                KoanMenuItem("Remove from History", KoanIcons.remove, destructive = true) { menu = false; forget() }
            }) { menu = false }
        }
    }
}

/**
 * When each play was, in the clock the phone is set to, as a column as wide as
 * the widest time the locale prints, so every title lines up.
 */
private class PlayClock(private val format: java.text.DateFormat, private val width: Dp) {
    fun time(epochSeconds: Long): String = format.format(Date(epochSeconds * 1000))

    @Composable
    fun Lead(epochSeconds: Long) {
        KoanText(time(epochSeconds), Modifier.width(width), role = KoanType.Fine, tone = KoanTone.Muted, maxLines = 1, textAlign = TextAlign.End)
    }
}

/**
 * The phone's own clock, 12- or 24-hour as its settings say, which
 * `DateTimeFormatter` would ignore. The column is measured once, from every
 * shape a time can take: a minute to each hour, its digits the locale's zero,
 * since digits are monospaced.
 */
@Composable
private fun rememberPlayClock(): PlayClock {
    val context = LocalContext.current
    val measurer = rememberTextMeasurer()
    val style = KoanType.Fine.style
    val density = LocalDensity.current
    val hours24 = DateFormat.is24HourFormat(context)
    // The format holds the zone it was made in, and the days are grouped in the current one.
    val zone = ZoneId.systemDefault()
    return remember(context, hours24, zone, style, density) {
        val format = DateFormat.getTimeFormat(context)
        val zero = DecimalFormatSymbols.getInstance().zeroDigit
        val widest = (0..23).maxOf { hour ->
            val at = Calendar.getInstance().apply { set(2001, 0, 1, hour, 59, 0) }
            val shape = format.format(at.time).map { if (it.isDigit()) zero else it }.joinToString("")
            measurer.measure(shape, style, maxLines = 1).size.width
        }
        PlayClock(format, with(density) { widest.toDp() })
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

/** "Today", "Yesterday", or the date in full. */
fun dayLabel(day: LocalDate, today: LocalDate): String = when (day) {
    today -> "Today"
    today.minusDays(1) -> "Yesterday"
    else -> day.format(dayFormat)
}

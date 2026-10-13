package cc.blit.koan.ui.pages

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanShape
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.theme.KoanTone
import cc.blit.koan.ui.Sleeve
import java.time.LocalDate
import java.time.ZoneId
import java.util.Locale
import uniffi.koan_ffi.PlayHistoryEntry

/**
 * Play history, grouped by day (Section.PlayHistory). The port of
 * `HistoryView.swift`: a log of events, not of tracks, so a record played three
 * times is three rows. The only grouping is the day. Tapping a row plays it;
 * a long press starts picking plays to forget, which erases the log and never
 * the tracks. Nothing here reorders.
 */
@Composable
fun PlayHistoryPage(runtime: KoanRuntime) {
    val entries by runtime.library.visiblePlayHistory.collectAsStateWithLifecycle()
    val filter by runtime.library.filter.collectAsStateWithLifecycle()
    val library = runtime.library
    val player = runtime.player
    val navigator = runtime.navigator
    PlayHistoryContent(
        entries = entries,
        filter = filter,
        onFilter = library::setFilter,
        onPlay = { player.playNow(listOf(it)) },
        onOpenAlbum = { navigator.openAlbum(it) },
        onOpenArtist = { navigator.openArtist(it) },
        onForget = library::forgetPlays,
        onClear = library::clearPlayHistory,
    )
}

/** The page over plain values, so it composes in a test with no runtime behind it. */
@Composable
internal fun PlayHistoryContent(
    entries: List<PlayHistoryEntry>,
    filter: String,
    onFilter: (String) -> Unit,
    onPlay: (trackId: Long) -> Unit,
    onOpenAlbum: (Long) -> Unit,
    onOpenArtist: (Long) -> Unit,
    onForget: (Set<Long>) -> Unit,
    onClear: () -> Unit,
    zone: ZoneId = ZoneId.systemDefault(),
    today: LocalDate = LocalDate.now(zone),
) {
    val locale = LocalConfiguration.current.locales[0] ?: Locale.getDefault()
    val lines = remember(entries, zone) {
        HistoryDays.lines(entries, id = { it.id }, playedAt = { it.playedAt }, zone = zone)
    }
    var picked by remember { mutableStateOf(emptySet<Long>()) }
    // A forgotten play, or one a filter hides, is no longer picked.
    val selection = remember(picked, entries) { HistorySelection.retain(picked, entries.map { it.id }) }
    var confirmingClear by remember { mutableStateOf(false) }
    val picking = selection.isNotEmpty()

    Column(Modifier.fillMaxSize()) {
        Column(
            Modifier.padding(horizontal = KoanSpace.page).padding(top = KoanSpace.l),
            verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
        ) {
            when {
                picking -> Row(verticalAlignment = Alignment.CenterVertically) {
                    KoanText(
                        "${selection.size} selected", KoanRole.Control, KoanTone.Strong,
                        modifier = Modifier.weight(1f),
                    )
                    KoanButton("done", { picked = emptySet() }, kind = KoanButtonKind.Text)
                    KoanButton(
                        "forget", { onForget(selection); picked = emptySet() },
                        kind = KoanButtonKind.Destructive,
                    )
                }
                confirmingClear -> Column {
                    KoanText("Clear history? Every play is forgotten. This cannot be undone.", KoanRole.Meta, KoanTone.Muted, maxLines = 3)
                    Row {
                        KoanButton("cancel", { confirmingClear = false }, kind = KoanButtonKind.Text)
                        KoanButton("clear", { confirmingClear = false; onClear() }, kind = KoanButtonKind.Destructive)
                    }
                }
                else -> Row(verticalAlignment = Alignment.CenterVertically) {
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
                        KoanText("History", KoanRole.Title, KoanTone.Strong)
                        KoanText(if (entries.size == 1) "1 play" else "${entries.size} plays", KoanRole.Control, KoanTone.Muted)
                    }
                    KoanButton("clear…", { confirmingClear = true }, kind = KoanButtonKind.Text, enabled = entries.isNotEmpty())
                }
            }
            HistoryFilterField(filter, onFilter)
        }
        Spacer(Modifier.size(KoanSpace.s))
        if (entries.isEmpty()) {
            Column(Modifier.fillMaxSize().padding(KoanSpace.page), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                KoanText(if (filter.isEmpty()) "Nothing played yet" else "No matches", KoanRole.Body, KoanTone.Strong)
                if (filter.isEmpty()) {
                    KoanText("Everything you play lands here, most recent first.", KoanRole.Meta, KoanTone.Muted, maxLines = 3)
                }
            }
        } else {
            LazyColumn(Modifier.fillMaxSize()) {
                items(lines, key = { it.key }, contentType = { if (it is HistoryLine.Heading) "heading" else "play" }) { line ->
                    when (line) {
                        is HistoryLine.Heading -> DayHeading(HistoryDays.heading(line.day, today, locale))
                        is HistoryLine.Play<*> -> {
                            val entry = line.item as PlayHistoryEntry
                            HistoryRow(
                                entry = entry,
                                time = HistoryDays.time(entry.playedAt, zone, locale),
                                picking = picking,
                                picked = entry.id in selection,
                                onTap = {
                                    if (picking) picked = HistorySelection.toggle(selection, entry.id) else onPlay(entry.track.id)
                                },
                                onLongPress = { picked = HistorySelection.toggle(selection, entry.id) },
                                onOpenAlbum = onOpenAlbum,
                                onOpenArtist = onOpenArtist,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun DayHeading(text: String) {
    Column(Modifier.fillMaxWidth().padding(top = KoanSpace.l)) {
        KoanText(text, KoanRole.Meta, KoanTone.Muted, modifier = Modifier.padding(horizontal = KoanSpace.page, vertical = KoanSpace.xs))
        KoanRule()
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun HistoryRow(
    entry: PlayHistoryEntry,
    time: String,
    picking: Boolean,
    picked: Boolean,
    onTap: () -> Unit,
    onLongPress: () -> Unit,
    onOpenAlbum: (Long) -> Unit,
    onOpenArtist: (Long) -> Unit,
) {
    val track = entry.track
    Row(
        Modifier
            .fillMaxWidth()
            .background(if (picked) KoanTheme.colors.surface else KoanTheme.colors.bg)
            .combinedClickable(onClick = onTap, onLongClick = onLongPress)
            .semantics { selected = picked }
            .sizeIn(minHeight = KoanShape.hit)
            .padding(horizontal = KoanSpace.page, vertical = KoanSpace.xs),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        if (picking) PickBox(picked)
        KoanText(time, KoanRole.Fine, KoanTone.Muted, data = true, modifier = Modifier.width(52.dp))
        // The cover is what you recognise a record by.
        val albumId = track.albumId
        if (albumId != null) Sleeve(ArtworkSource.Album(albumId), SLEEVE) else Box(Modifier.size(SLEEVE).background(KoanTheme.colors.surface))
        Column(Modifier.weight(1f)) {
            KoanText(track.title, KoanRole.Body, KoanTone.Ink, data = true)
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                val artistId = track.artistId
                KoanText(
                    track.artistName, KoanRole.Fine, KoanTone.Muted, data = true,
                    modifier = Modifier.weight(1f, fill = false).then(
                        if (artistId != null && !picking) Modifier.clickable { onOpenArtist(artistId) } else Modifier,
                    ),
                )
                KoanText("·", KoanRole.Fine, KoanTone.Muted)
                KoanText(
                    track.albumTitle, KoanRole.Fine, KoanTone.Muted, data = true,
                    modifier = Modifier.weight(1f, fill = false).then(
                        if (albumId != null && !picking) Modifier.clickable { onOpenAlbum(albumId) } else Modifier,
                    ),
                )
            }
        }
        // A play recorded by another client scrobbling in did not happen here.
        if (entry.source != "local") {
            KoanIcon("renderer", tone = KoanTone.Muted, textSizeDp = 12f, description = "Scrobbled by another client")
        }
        track.durationMs?.let { KoanText(formatDuration(it), KoanRole.Fine, KoanTone.Muted, data = true) }
    }
}

/** The tick box of a picked row: the toggle's box, 14 dp, accent when on. */
@Composable
private fun PickBox(on: Boolean) {
    val colors = KoanTheme.colors
    Box(
        Modifier.size(14.dp).then(
            if (on) Modifier.background(colors.accent) else Modifier.border(BorderStroke(KoanShape.hairline, colors.muted), RectangleShape),
        ),
        contentAlignment = Alignment.Center,
    ) {
        if (on) KoanIcon("check", tone = KoanTone.Ink, textSizeDp = 10f)
    }
}

/**
 * A plain filter field: the text in `control` type over a hairline. Private to
 * this page until the theme has a field of its own; the other filtered pages
 * will want the same one.
 */
@Composable
private fun HistoryFilterField(value: String, onChange: (String) -> Unit) {
    // The model debounces and may answer with its own text; the field keeps what is typed.
    var text by remember { mutableStateOf(value) }
    LaunchedEffect(value) { if (value != text) text = value }
    val colors = KoanTheme.colors
    Column(Modifier.fillMaxWidth()) {
        Box(Modifier.fillMaxWidth().sizeIn(minHeight = KoanShape.hit), contentAlignment = Alignment.CenterStart) {
            if (text.isEmpty()) KoanText(Section.PlayHistory.filterPlaceholder.orEmpty(), KoanRole.Control, KoanTone.Muted)
            BasicTextField(
                value = text,
                onValueChange = { text = it; onChange(it) },
                singleLine = true,
                textStyle = KoanTheme.style(KoanRole.Control).copy(color = colors.ink),
                cursorBrush = SolidColor(colors.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                interactionSource = remember { MutableInteractionSource() },
                modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Filter history" },
            )
        }
        KoanRule()
    }
}

private val SLEEVE = 44.dp

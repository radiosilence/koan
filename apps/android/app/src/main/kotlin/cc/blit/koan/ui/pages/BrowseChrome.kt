package cc.blit.koan.ui.pages

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import cc.blit.koan.nav.Tab
import cc.blit.koan.state.BrowseFilterNone
import cc.blit.koan.state.activeCount
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanFilterField
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanRule
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanToggle
import cc.blit.koan.theme.KoanTone
import uniffi.koan_ffi.BrowseFilter

/*
 * What the three browsers share: restoring where a tab left a list, the
 * header (title, count, name filter, browse filters), a strip of choices, and
 * the empty state. Every control here calls a model method; none reaches the
 * engine.
 */

/**
 * The tab this page was composed in. A tab's stack keeps its routes but
 * composes only the tab in front, so the tab in front at composition is the
 * one the page belongs to; read once, never followed.
 */
@Composable
fun rememberHomeTab(runtime: KoanRuntime): Tab = remember { runtime.navigator.state.value.tab }

/** A grid put back where [tab] left [section], and noted as it moves. */
@Composable
fun rememberRestoredGridState(runtime: KoanRuntime, tab: Tab, section: Section): LazyGridState {
    val place = remember { ScrollStore.App[tab, section] }
    val state = rememberLazyGridState(place.index, place.offset)
    LaunchedEffect(state) {
        snapshotFlow { ScrollPlace(state.firstVisibleItemIndex, state.firstVisibleItemScrollOffset) }
            .collect { ScrollStore.App.put(tab, section, it) }
    }
    ScrollToTopOnRewind(runtime, tab) { state.scrollToItem(0) }
    return state
}

/** A list put back where [tab] left [section], and noted as it moves. */
@Composable
fun rememberRestoredListState(runtime: KoanRuntime, tab: Tab, section: Section): LazyListState {
    val place = remember { ScrollStore.App[tab, section] }
    val state = rememberLazyListState(place.index, place.offset)
    LaunchedEffect(state) {
        snapshotFlow { ScrollPlace(state.firstVisibleItemIndex, state.firstVisibleItemScrollOffset) }
            .collect { ScrollStore.App.put(tab, section, it) }
    }
    ScrollToTopOnRewind(runtime, tab) { state.scrollToItem(0) }
    return state
}

/** Runs [toTop] when the navigator sends [tab] back to its top; not for the count the page arrived with. */
@Composable
private fun ScrollToTopOnRewind(runtime: KoanRuntime, tab: Tab, toTop: suspend () -> Unit) {
    val rewinds by runtime.navigator.rewinds.collectAsStateWithLifecycle()
    val count = rewinds[tab] ?: 0
    var seen by remember { mutableIntStateOf(count) }
    LaunchedEffect(count) {
        if (count != seen) {
            seen = count
            toTop()
        }
    }
}

/**
 * The top of a browser: its title and, once narrowed, how many; the filters
 * button saying how many are on; the name filter; and whatever [sorts] draws.
 */
@Composable
fun BrowseHeader(
    runtime: KoanRuntime,
    section: Section,
    count: String?,
    sorts: @Composable () -> Unit = {},
) {
    val library = runtime.library
    val filter by library.filter.collectAsStateWithLifecycle()
    val browse by library.browseFilter.collectAsStateWithLifecycle()
    var open by rememberSaveable { mutableStateOf(false) }
    val on = browse.activeCount
    Column(Modifier.fillMaxWidth().padding(horizontal = KoanSpace.l).padding(top = KoanSpace.l), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                KoanText(section.title, KoanRole.TitleSm, KoanTone.Strong)
                if (count != null) KoanText(count, KoanRole.Fine, KoanTone.Muted)
            }
            KoanButton(
                if (on > 0) "filters ($on)" else "filters",
                onClick = {
                    open = !open
                    if (open) library.loadBrowseChoices()
                },
                kind = if (on > 0 || open) KoanButtonKind.Bordered else KoanButtonKind.Text,
                icon = "filter",
            )
        }
        NameField(filter, section.filterPlaceholder ?: "Filter", library::setFilter)
        sorts()
        if (open) FilterPanel(runtime, section)
    }
}

/**
 * The name filter. It keeps its own text while it has focus, and adopts the
 * model's only when it has not: a See all arrives with a filter in hand, and
 * the model's echo of a keystroke may be a keystroke behind the field.
 */
@Composable
private fun NameField(model: String, placeholder: String, onChange: (String) -> Unit) {
    var text by remember { mutableStateOf(model) }
    var focused by remember { mutableStateOf(false) }
    LaunchedEffect(model) {
        if (!focused && text != model) text = model
    }
    KoanFilterField(
        value = text,
        onValueChange = {
            text = it
            onChange(it)
        },
        placeholder = placeholder,
        modifier = Modifier.onFocusChanged { focused = it.isFocused },
    )
}

/** The browse filters, as `BrowseFilterForm` has them: toggles, codec, genre, years, reset. */
@Composable
private fun FilterPanel(runtime: KoanRuntime, section: Section) {
    val library = runtime.library
    val filter by library.browseFilter.collectAsStateWithLifecycle()
    val choices by library.browseChoices.collectAsStateWithLifecycle()
    fun set(next: BrowseFilter) = library.setBrowseFilter(next)
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        KoanRule()
        KoanToggle("Favourites", filter.favourites, { set(filter.copy(favourites = it)) })
        KoanToggle("Recently Played", filter.recent, { set(filter.copy(recent = it)) })
        KoanToggle("Downloaded", filter.downloaded, { set(filter.copy(downloaded = it)) })
        // A track's codec says this already, and the track listing filters by codec.
        if (section != Section.Tracks) KoanToggle("Lossless", filter.lossless, { set(filter.copy(lossless = it)) })
        ChoiceStrip(
            label = "Codec",
            options = offeredChoices(choices?.codecs, filter.codec).map { it to it },
            selected = filter.codec,
            onSelect = { set(filter.copy(codec = it)) },
            any = true,
        )
        ChoiceStrip(
            label = "Genre",
            options = offeredChoices(choices?.genres, filter.genre).map { it to it },
            selected = filter.genre,
            onSelect = { set(filter.copy(genre = it)) },
            any = true,
        )
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
            KoanText("Years", KoanRole.Body, modifier = Modifier.width(72.dp))
            YearBox("From", filter.yearFrom, Modifier.weight(1f)) { set(filter.copy(yearFrom = it)) }
            YearBox("To", filter.yearTo, Modifier.weight(1f)) { set(filter.copy(yearTo = it)) }
        }
        KoanButton(
            "reset", onClick = { set(BrowseFilterNone) }, enabled = filter.activeCount > 0,
            kind = KoanButtonKind.Bordered, modifier = Modifier.align(Alignment.End),
        )
        KoanRule()
    }
}

@Composable
private fun YearBox(prompt: String, value: Int?, modifier: Modifier, onChange: (Int?) -> Unit) {
    var text by remember { mutableStateOf(value?.toString().orEmpty()) }
    // Reset, from outside the field.
    LaunchedEffect(value) {
        if (value == null && text.length == 4) text = ""
    }
    KoanFilterField(
        value = text,
        onValueChange = {
            val typed = yearTyped(it)
            text = typed.text
            if (typed.apply) onChange(typed.value)
        },
        placeholder = prompt,
        modifier = modifier,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
        clearable = false,
    )
}

/**
 * A row of choices that scrolls sideways: the one on is bordered. With [any],
 * a first choice that clears it, as the pickers' "Any".
 */
@Composable
fun <T> ChoiceStrip(
    options: List<Pair<T, String>>,
    selected: T?,
    onSelect: (T?) -> Unit,
    modifier: Modifier = Modifier,
    label: String? = null,
    any: Boolean = false,
) {
    Row(modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        if (label != null) KoanText(label, KoanRole.Body, modifier = Modifier.width(72.dp))
        Row(Modifier.weight(1f).horizontalScroll(rememberScrollState())) {
            if (any) {
                KoanButton("any", { onSelect(null) }, kind = if (selected == null) KoanButtonKind.Bordered else KoanButtonKind.Text)
            }
            for ((value, name) in options) {
                KoanButton(
                    name, { onSelect(value) },
                    kind = if (value == selected) KoanButtonKind.Bordered else KoanButtonKind.Text,
                    data = true,
                )
            }
        }
    }
}

/** Nothing to list: a glyph, what is missing, and why or what to do. */
@Composable
fun EmptyNote(icon: String, title: String, detail: String?, modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxSize().padding(KoanSpace.page),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        KoanIcon(icon, tone = KoanTone.Muted, textSizeDp = 32f)
        KoanText(title, KoanRole.TitleSm, KoanTone.Muted)
        if (detail != null) KoanText(detail, KoanRole.Meta, KoanTone.Muted, maxLines = 4)
    }
}

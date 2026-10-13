package cc.blit.koan.ui.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.unit.dp
import cc.blit.koan.ffi.BrowseChoices
import cc.blit.koan.ffi.BrowseFilter
import cc.blit.koan.ui.playlists.MenuItem
import cc.blit.koan.ui.theme.KoanBadge
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanEdge
import cc.blit.koan.ui.theme.KoanField
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanToggle
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.LocalKoanTokens
import cc.blit.koan.ui.theme.koanChip
import cc.blit.koan.ui.theme.koanRule

/** The sort a listing is in, and the others it offers, behind one button. */
@Composable
fun <S> SortButton(selected: S, offered: List<S>, label: (S) -> String, onSort: (S) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        KoanButton("Sort", { open = true }, kind = KoanButtonKind.Icon, icon = KoanIcons.sort)
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            offered.forEach { sort ->
                MenuItem(label(sort), if (sort == selected) KoanIcons.check else null) {
                    open = false
                    onSort(sort)
                }
            }
        }
    }
}

/**
 * The browsers' filters, behind one button that says how many are on: a
 * narrowed listing otherwise looks like a missing library. The same filters as
 * the Apple apps', answered by the same query.
 */
@Composable
fun FilterButton(filter: BrowseFilter, choices: BrowseChoices?, onFilter: (BrowseFilter) -> Unit, lossless: Boolean = true) {
    var open by rememberSaveable { mutableStateOf(false) }
    KoanButton({ open = true }, kind = KoanButtonKind.Icon) {
        KoanIcon(KoanIcons.filters, "Filters")
        if (filter.activeCount > 0) KoanBadge("${filter.activeCount}", Modifier.padding(start = KoanSpace.xs), accent = true)
    }
    if (open) FilterSheet(filter, choices, onFilter, lossless) { open = false }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FilterSheet(filter: BrowseFilter, choices: BrowseChoices?, onFilter: (BrowseFilter) -> Unit, lossless: Boolean, onDismiss: () -> Unit) {
    val gutter = LocalKoanTokens.current.gutter
    ModalBottomSheet(onDismissRequest = onDismiss, shape = RectangleShape, containerColor = KoanTheme.colors.bg) {
        Column(
            Modifier
                .verticalScroll(rememberScrollState())
                .padding(horizontal = gutter)
                .navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(KoanSpace.xs),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                KoanText("Filters", Modifier.weight(1f), role = KoanType.TitleSmall, appText = true)
                KoanButton("Clear", { onFilter(NoFilter) }, kind = KoanButtonKind.Text, enabled = filter.activeCount > 0)
            }
            KoanToggle("Favourites", filter.favourites, { onFilter(filter.copy(favourites = it)) })
            KoanToggle("Recently Played", filter.recent, { onFilter(filter.copy(recent = it)) })
            KoanToggle("Downloaded", filter.downloaded, { onFilter(filter.copy(downloaded = it)) })
            if (lossless) KoanToggle("Lossless", filter.lossless, { onFilter(filter.copy(lossless = it)) })
            Choices("Format", choices?.codecs.orEmpty(), filter.codec, { it.uppercase() }) { onFilter(filter.copy(codec = it)) }
            Choices("Genre", choices?.genres.orEmpty(), filter.genre, { it }) { onFilter(filter.copy(genre = it)) }
            KoanSectionHeader("Year")
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
                YearField("From", filter.yearFrom, Modifier.weight(1f)) { onFilter(filter.copy(yearFrom = it)) }
                YearField("To", filter.yearTo, Modifier.weight(1f)) { onFilter(filter.copy(yearTo = it)) }
            }
            Box(Modifier.padding(bottom = gutter))
        }
    }
}

/** "Any", then the library's own values; the current one kept even when the library no longer has it. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Choices(title: String, values: List<String>, selected: String?, shown: (String) -> String, onChoose: (String?) -> Unit) {
    val all = if (selected != null && selected !in values) listOf(selected) + values else values
    if (all.isEmpty()) return
    KoanSectionHeader(title)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        Chip("Any", selected == null, appText = true) { onChoose(null) }
        all.forEach { value -> Chip(shown(value), value == selected) { onChoose(value) } }
    }
}

@Composable
private fun Chip(label: String, on: Boolean, appText: Boolean = false, onClick: () -> Unit) {
    KoanText(
        label,
        Modifier
            .koanChip()
            .clickable(remember { MutableInteractionSource() }, indication = null, onClick = onClick),
        role = KoanType.Control,
        tone = if (on) KoanTone.Accent else KoanTone.Ink,
        appText = appText,
    )
}

/**
 * A year, applied as it is typed: closing the sheet with the field focused
 * would otherwise drop it. Only a whole year or an empty field is applied, so
 * typing 1990 is one query rather than four.
 */
@Composable
private fun YearField(label: String, year: Int?, modifier: Modifier, onYear: (Int?) -> Unit) {
    var text by rememberSaveable { mutableStateOf(year?.toString().orEmpty()) }
    KoanField(
        text,
        { typed ->
            text = typed.filter(Char::isDigit).take(4)
            when {
                text.isEmpty() -> onYear(null)
                text.length == 4 -> onYear(text.toInt())
            }
        },
        modifier,
        placeholder = label,
    )
}

/** Narrows a listing by name as it is typed: a glyph, the field, and a clear button. */
@Composable
fun FilterField(text: String, placeholder: String, onText: (String) -> Unit, modifier: Modifier = Modifier) {
    Row(
        modifier
            .fillMaxWidth()
            .koanRule(KoanEdge.Bottom)
            .padding(horizontal = LocalKoanTokens.current.gutter, vertical = KoanSpace.xs),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        KoanIcon(KoanIcons.search, null, color = KoanTheme.color(KoanTone.Muted))
        KoanField(text, onText, Modifier.weight(1f).padding(vertical = 2.dp), placeholder = placeholder, ground = false)
        if (text.isNotEmpty()) KoanButton("Clear", { onText("") }, kind = KoanButtonKind.Icon, icon = KoanIcons.clearField)
    }
}

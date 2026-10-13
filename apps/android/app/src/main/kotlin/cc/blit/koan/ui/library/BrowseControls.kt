package cc.blit.koan.ui.library

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
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import cc.blit.koan.R
import cc.blit.koan.ffi.BrowseChoices
import cc.blit.koan.ffi.BrowseFilter
import cc.blit.koan.ui.theme.LocalKoanTokens

/** The sort a listing is in, and the others it offers, behind one button. */
@Composable
fun <S> SortButton(selected: S, offered: List<S>, label: (S) -> String, onSort: (S) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(painterResource(R.drawable.ic_sort), "Sort") }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            offered.forEach { sort ->
                DropdownMenuItem(
                    text = { Text(label(sort)) },
                    trailingIcon = { if (sort == selected) Text("✓") },
                    onClick = {
                        open = false
                        onSort(sort)
                    },
                )
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
    IconButton(onClick = { open = true }) {
        BadgedBox(badge = { if (filter.activeCount > 0) Badge { Text("${filter.activeCount}") } }) {
            Icon(painterResource(R.drawable.ic_filter), "Filters")
        }
    }
    if (open) FilterSheet(filter, choices, onFilter, lossless) { open = false }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun FilterSheet(filter: BrowseFilter, choices: BrowseChoices?, onFilter: (BrowseFilter) -> Unit, lossless: Boolean, onDismiss: () -> Unit) {
    val gutter = LocalKoanTokens.current.gutter
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            Modifier
                .verticalScroll(rememberScrollState())
                .padding(horizontal = gutter)
                .navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Filters", style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                TextButton(onClick = { onFilter(NoFilter) }, enabled = filter.activeCount > 0) { Text("Clear") }
            }
            Toggle("Favourites", filter.favourites) { onFilter(filter.copy(favourites = it)) }
            Toggle("Recently Played", filter.recent) { onFilter(filter.copy(recent = it)) }
            Toggle("Downloaded", filter.downloaded) { onFilter(filter.copy(downloaded = it)) }
            if (lossless) Toggle("Lossless", filter.lossless) { onFilter(filter.copy(lossless = it)) }
            Choices("Format", choices?.codecs.orEmpty(), filter.codec, { it.uppercase() }) { onFilter(filter.copy(codec = it)) }
            Choices("Genre", choices?.genres.orEmpty(), filter.genre, { it }) { onFilter(filter.copy(genre = it)) }
            Text("Year", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                YearField("From", filter.yearFrom, Modifier.weight(1f)) { onFilter(filter.copy(yearFrom = it)) }
                YearField("To", filter.yearTo, Modifier.weight(1f)) { onFilter(filter.copy(yearTo = it)) }
            }
            Box(Modifier.padding(bottom = gutter))
        }
    }
}

@Composable
private fun Toggle(label: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        Switch(checked = on, onCheckedChange = onChange)
    }
}

/** "Any", then the library's own values; the current one kept even when the library no longer has it. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Choices(title: String, values: List<String>, selected: String?, shown: (String) -> String, onChoose: (String?) -> Unit) {
    val all = if (selected != null && selected !in values) listOf(selected) + values else values
    if (all.isEmpty()) return
    Text(title, style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 12.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        FilterChip(selected = selected == null, onClick = { onChoose(null) }, label = { Text("Any") })
        all.forEach { value ->
            FilterChip(selected = value == selected, onClick = { onChoose(value) }, label = { Text(shown(value)) })
        }
    }
}

/**
 * A year, applied as it is typed: closing the sheet with the field focused
 * would otherwise drop it. Only a whole year or an empty field is applied, so
 * typing 1990 is one query rather than four.
 */
@Composable
private fun YearField(label: String, year: Int?, modifier: Modifier, onYear: (Int?) -> Unit) {
    var text by rememberSaveable { mutableStateOf(year?.toString().orEmpty()) }
    OutlinedTextField(
        value = text,
        onValueChange = { typed ->
            text = typed.filter(Char::isDigit).take(4)
            when {
                text.isEmpty() -> onYear(null)
                text.length == 4 -> onYear(text.toInt())
            }
        },
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
        modifier = modifier,
    )
}

/** Narrows a listing by name as it is typed. */
@Composable
fun FilterField(text: String, placeholder: String, onText: (String) -> Unit, modifier: Modifier = Modifier) {
    val gutter = LocalKoanTokens.current.gutter
    OutlinedTextField(
        value = text,
        onValueChange = onText,
        placeholder = { Text(placeholder) },
        leadingIcon = { Icon(painterResource(R.drawable.ic_search), null) },
        trailingIcon = {
            if (text.isNotEmpty()) TextButton(onClick = { onText("") }) { Text("Clear") }
        },
        singleLine = true,
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
        shape = MaterialTheme.shapes.extraLarge,
        modifier = modifier.fillMaxWidth().padding(horizontal = gutter, vertical = 4.dp),
    )
}

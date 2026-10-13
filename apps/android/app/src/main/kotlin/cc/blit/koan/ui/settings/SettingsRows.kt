package cc.blit.koan.ui.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import cc.blit.koan.ui.theme.LocalKoanTokens

/**
 * A settings page: its title, its sections, and the status line beneath them
 * saying what the last action came to.
 */
@Composable
fun SettingsPage(title: String, state: SettingsState, content: LazyListScope.() -> Unit) {
    val gutter = LocalKoanTokens.current.gutter
    Column(Modifier.fillMaxSize()) {
        LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
            item {
                Text(
                    title,
                    style = MaterialTheme.typography.headlineMedium,
                    modifier = Modifier.padding(horizontal = gutter, vertical = 12.dp),
                )
            }
            content()
        }
        StatusLine(state)
    }
}

/**
 * The result of the last action, or the reason it failed. Takes no room until
 * there is something to say: an action that reports nothing looks like it did
 * nothing.
 */
@Composable
private fun StatusLine(state: SettingsState) {
    val text = state.error ?: state.result ?: return
    Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
        Text(
            text,
            color = if (state.error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
            maxLines = 3,
            modifier = Modifier.fillMaxWidth().padding(horizontal = LocalKoanTokens.current.gutter, vertical = 8.dp),
        )
    }
}

/** A section's heading. */
fun LazyListScope.header(text: String) = item {
    Text(
        text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = LocalKoanTokens.current.gutter, end = LocalKoanTokens.current.gutter, top = 20.dp, bottom = 4.dp),
    )
}

/** What a section is for, under it. */
fun LazyListScope.footer(text: String) = item {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = LocalKoanTokens.current.gutter, vertical = 4.dp),
    )
}

/** A value with its label, read only. */
@Composable
fun ValueRow(label: String, value: String, onClick: (() -> Unit)? = null) {
    ListItem(
        headlineContent = { Text(label) },
        trailingContent = { Text(value, color = MaterialTheme.colorScheme.onSurfaceVariant) },
        modifier = if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier,
    )
}

@Composable
fun SwitchRow(label: String, checked: Boolean, enabled: Boolean = true, detail: String? = null, onChange: (Boolean) -> Unit) {
    ListItem(
        headlineContent = { Text(label) },
        supportingContent = detail?.let { { Text(it) } },
        trailingContent = { Switch(checked, onChange, enabled = enabled) },
        modifier = Modifier.clickable(enabled = enabled) { onChange(!checked) },
    )
}

/** One of a few choices, shown as the chosen one's label and picked from a menu. */
@Composable
fun <T> ChoiceRow(label: String, selected: T, options: List<Pair<String, T>>, onChange: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        ListItem(
            headlineContent = { Text(label) },
            trailingContent = {
                Text(options.firstOrNull { it.second == selected }?.first ?: "", color = MaterialTheme.colorScheme.primary)
            },
            modifier = Modifier.clickable { open = true },
        )
        DropdownMenu(open, { open = false }, Modifier.align(Alignment.TopEnd)) {
            options.forEach { (name, value) ->
                DropdownMenuItem(text = { Text(name) }, onClick = {
                    open = false
                    onChange(value)
                })
            }
        }
    }
}

/** A row of buttons inside a section, laid out from the start. */
@Composable
fun ButtonRow(content: @Composable RowScope.() -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = LocalKoanTokens.current.gutter, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        content = content,
    )
}

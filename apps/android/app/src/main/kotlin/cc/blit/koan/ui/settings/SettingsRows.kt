package cc.blit.koan.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import cc.blit.koan.ui.theme.KoanEdge
import cc.blit.koan.ui.theme.KoanFormRow
import cc.blit.koan.ui.theme.KoanPicker
import cc.blit.koan.ui.theme.KoanSectionHeader
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanToggle
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.koanRule
import cc.blit.koan.ui.theme.koanSurface

/**
 * A settings page: its title, its sections, and the status line beneath them
 * saying what the last action came to.
 */
@Composable
fun SettingsPage(title: String, state: SettingsState, content: LazyListScope.() -> Unit) {
    Column(Modifier.fillMaxSize().koanSurface()) {
        LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
            item {
                KoanText(
                    title,
                    Modifier.padding(horizontal = KoanSpace.page, vertical = KoanSpace.m),
                    role = KoanType.Title,
                    tone = KoanTone.Strong,
                    appText = true,
                )
            }
            content()
            item { Spacer(Modifier.height(KoanSpace.xl)) }
        }
        StatusLine(state)
    }
}

/**
 * The result of the last action, or the reason it failed. Takes no room until
 * there is something to say, then sits on the ground under a rule: an action
 * that reports nothing looks like it did nothing.
 */
@Composable
private fun StatusLine(state: SettingsState) {
    val text = state.error ?: state.result ?: return
    KoanText(
        text,
        Modifier
            .fillMaxWidth()
            .koanSurface()
            .koanRule(KoanEdge.Top)
            .padding(horizontal = KoanSpace.page, vertical = KoanSpace.s),
        role = KoanType.Fine,
        tone = if (state.error != null) KoanTone.Bad else KoanTone.Muted,
        maxLines = 3,
    )
}

/** Rows inside a section keep the page margin. */
internal val inset = Modifier.padding(horizontal = KoanSpace.page)

/** A section's heading. */
fun LazyListScope.header(text: String) = item { KoanSectionHeader(text, inset) }

/** What a section is for, under it. */
fun LazyListScope.footer(text: String) = item {
    KoanText(text, inset.padding(vertical = KoanSpace.xs), role = KoanType.Fine, tone = KoanTone.Muted)
}

/** A line of words in a section, in `tone`. */
fun LazyListScope.line(text: String, tone: KoanTone = KoanTone.Ink) = item {
    KoanText(text, inset.padding(vertical = KoanSpace.s), tone = tone)
}

/** A value with its label, read only. Values are data, so they keep their case. */
@Composable
fun ValueRow(label: String, value: String, trailing: (@Composable () -> Unit)? = null) {
    KoanFormRow(label, inset) {
        Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
            KoanText(value, Modifier.weight(1f), role = KoanType.Control, tone = KoanTone.Muted, maxLines = 2)
            trailing?.invoke()
        }
    }
}

@Composable
fun SwitchRow(label: String, checked: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    KoanToggle(label, checked, onChange, inset, enabled)
}

/** One of a few choices, picked from a menu. */
@Composable
fun <T> ChoiceRow(label: String, selected: T, options: List<Pair<String, T>>, onChange: (T) -> Unit) {
    KoanFormRow(label, inset) { KoanPicker(options, selected, onChange) }
}

/** A row of buttons inside a section, laid out from the start. */
@Composable
fun ButtonRow(content: @Composable RowScope.() -> Unit) {
    Row(
        inset.fillMaxWidth().padding(vertical = KoanSpace.s),
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        verticalAlignment = Alignment.CenterVertically,
        content = content,
    )
}

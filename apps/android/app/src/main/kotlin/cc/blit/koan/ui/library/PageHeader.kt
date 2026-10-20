package cc.blit.koan.ui.library

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.FlowRowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/**
 * The head of a record's, an artist's or a playlist's page, one layout for
 * all three ("Shared edges" in `docs/design/koan-theme.md`): the cover, then
 * the title, the lines of detail under it and the page's actions, every one
 * on the gutter's edge, as the iPhone's stacked header has them. The actions
 * wrap rather than clip on a narrow phone; `below` is whatever follows them.
 */
@Composable
fun PageHeader(
    title: String,
    modifier: Modifier = Modifier,
    art: (@Composable () -> Unit)? = null,
    detail: @Composable ColumnScope.() -> Unit = {},
    actions: @Composable FlowRowScope.() -> Unit = {},
    below: @Composable ColumnScope.() -> Unit = {},
) {
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
        if (art != null) Box(Modifier.padding(bottom = KoanSpace.m)) { art() }
        KoanText(title, role = KoanType.Title, tone = KoanTone.Strong, maxLines = 3)
        detail()
        FlowRow(
            Modifier.padding(vertical = KoanSpace.m),
            horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
            itemVerticalAlignment = Alignment.CenterVertically,
        ) {
            actions()
        }
        below()
    }
}

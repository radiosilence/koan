package cc.blit.koan.ui.pages

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone

/**
 * The head of a record, an artist or (later) a playlist: the cover as wide as
 * the first column of the grid beneath it, the text starting on the second,
 * and the actions in a wrapping row across the full width below. Without a
 * cover (an artist with no photograph) the text starts at the margin.
 *
 * [inset] is the page's side margin, which a header inside a padded grid sets to
 * zero (the grid has already applied it); the cover is sized by the full width
 * either way. [columns] is the page's grid column count (see [gridColumns]); it is passed in
 * so the cover and the grid cannot disagree about how wide a column is.
 *
 * Shared by the album, artist and playlist pages. Slots rather than fields
 * because what sits under a title differs: an artist link, details, a count.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun RecordHeader(
    title: String,
    columns: Int,
    modifier: Modifier = Modifier,
    inset: Dp = KoanSpace.page,
    cover: (@Composable (size: Dp) -> Unit)? = null,
    subtitle: @Composable () -> Unit = {},
    actions: @Composable () -> Unit = {},
) {
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val column = tileWidth(maxWidth + inset * 2, columns)
        Column(Modifier.padding(horizontal = inset)) {
            Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.l), verticalAlignment = Alignment.Top) {
                if (cover != null) cover(column)
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                    KoanText(displayTitle(title), KoanRole.TitleSm, KoanTone.Strong, data = true, maxLines = 4)
                    subtitle()
                }
            }
            FlowRow(
                Modifier.fillMaxWidth().padding(top = KoanSpace.m),
                horizontalArrangement = Arrangement.spacedBy(KoanSpace.xs),
                itemVerticalAlignment = Alignment.CenterVertically,
            ) { actions() }
        }
    }
}

package cc.blit.koan.ui.settings

import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanMenu
import cc.blit.koan.ui.theme.KoanMenuItem
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/**
 * The EQ presets an output can be set from, as iOS's `PresetMenu` offers
 * them: Flat, or one of the presets, the one it was set from ticked. The
 * button says what it is set to, in the accent while what is heard is
 * processed.
 */
@Composable
fun PresetButton(
    summary: String,
    presets: List<String>,
    current: String?,
    tinted: Boolean,
    choose: (String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    var open by remember { mutableStateOf(false) }
    Box(modifier) {
        KoanButton({ open = true }, Modifier.semantics { contentDescription = "Preset: $summary" }, kind = KoanButtonKind.Text) {
            KoanIcon(KoanIcons.filters, contentDescription = null, color = KoanTheme.color(if (tinted) KoanTone.Accent else KoanTone.Ink))
            KoanText(summary, Modifier.weight(1f, fill = false), role = KoanType.Control, tone = KoanTone.Ink, maxLines = 1)
        }
        KoanMenu(open, onDismissRequest = { open = false }) {
            (listOf<Pair<String, String?>>("Flat" to null) + presets.map { it to it }).forEach { (label, preset) ->
                KoanMenuItem(label, icon = if (preset == current) KoanIcons.check else null) {
                    open = false
                    choose(preset)
                }
            }
        }
    }
}

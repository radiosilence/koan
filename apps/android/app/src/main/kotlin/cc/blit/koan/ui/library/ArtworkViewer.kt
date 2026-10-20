package cc.blit.koan.ui.library

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import cc.blit.koan.art.Art
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType

/**
 * A cover at full size, the whole of it in view, with its title and a line
 * under it. A tap anywhere or back dismisses it: there is no close button on
 * a picture.
 */
@Composable
fun ArtworkViewer(art: Art, title: String, subtitle: String?, onDismiss: () -> Unit) {
    Dialog(onDismiss, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Box(
            Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background)
                .clickable(remember { MutableInteractionSource() }, indication = null, onClickLabel = "Close", onClick = onDismiss)
                .safeDrawingPadding()
                .padding(KoanSpace.xl),
            contentAlignment = Alignment.Center,
        ) {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(KoanSpace.xs)) {
                // The largest square the caption leaves room for.
                BoxWithConstraints(Modifier.weight(1f, fill = false)) {
                    Sleeve(art, Modifier.padding(bottom = KoanSpace.l), size = minOf(maxWidth, maxHeight - KoanSpace.l).coerceAtLeast(0.dp))
                }
                KoanText(title, role = KoanType.TitleSmall, tone = KoanTone.Strong, textAlign = TextAlign.Center, maxLines = 2)
                if (!subtitle.isNullOrEmpty()) {
                    KoanText(subtitle, role = KoanType.Meta, tone = KoanTone.Muted, textAlign = TextAlign.Center, maxLines = 2)
                }
            }
        }
    }
}

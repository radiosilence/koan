package cc.blit.koan.theme

import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

/**
 * A seek bar in the theme's own terms: a two-point `rule` track, the part that
 * can be reached in `muted`, the part played in `accent`, and a square thumb
 * that appears only under a finger. Flat and square, like everything else.
 *
 * [progress] is a function, read only while drawing, so a bar that moves with
 * the music invalidates its draw and never recomposes. [available] is how far
 * into the track a seek can land (below 1 while a download is still arriving).
 *
 * A drag calls [onScrub] with each position (0 to 1) so the caller can hold
 * the head where the finger is; letting go calls [onCommit] with the last one.
 * A tap is a scrub and a commit at once. A position past [available] is
 * reported as it was touched: the caller clamps, and explains if it must.
 */
@Composable
fun KoanSlider(
    progress: () -> Float,
    onScrub: (Float) -> Unit,
    onCommit: (Float) -> Unit,
    modifier: Modifier = Modifier,
    available: Float = 1f,
    enabled: Boolean = true,
    description: String = "Seek",
) {
    val colors = KoanTheme.colors
    val scrub by rememberUpdatedState(onScrub)
    val commit by rememberUpdatedState(onCommit)
    var dragging by remember { mutableStateOf(false) }
    var last by remember { mutableStateOf(0f) }
    Box(
        modifier
            .fillMaxWidth()
            .height(KoanShape.hit)
            .alpha(if (enabled) 1f else 0.4f)
            .semantics {
                contentDescription = description
                progressBarRangeInfo = ProgressBarRangeInfo(progress().coerceIn(0f, 1f), 0f..1f)
            }
            .pointerInput(enabled) {
                if (!enabled) return@pointerInput
                detectTapGestures { at ->
                    val f = (at.x / size.width).coerceIn(0f, 1f)
                    scrub(f)
                    commit(f)
                }
            }
            .pointerInput(enabled) {
                if (!enabled) return@pointerInput
                detectHorizontalDragGestures(
                    onDragStart = { at ->
                        dragging = true
                        last = (at.x / size.width).coerceIn(0f, 1f)
                        scrub(last)
                    },
                    onDragEnd = {
                        dragging = false
                        commit(last)
                    },
                    onDragCancel = { dragging = false },
                    onHorizontalDrag = { change, _ ->
                        last = (change.position.x / size.width).coerceIn(0f, 1f)
                        scrub(last)
                    },
                )
            }
            .drawBehind {
                val track = 2.dp.toPx()
                val y = (size.height - track) / 2
                drawRect(colors.rule, Offset(0f, y), Size(size.width, track))
                val reach = available.coerceIn(0f, 1f)
                if (reach < 1f) drawRect(colors.muted, Offset(0f, y), Size(size.width * reach, track))
                val played = size.width * progress().coerceIn(0f, 1f)
                drawRect(colors.accent, Offset(0f, y), Size(played, track))
                if (dragging) {
                    val thumb = 12.dp.toPx()
                    drawRect(colors.accent, Offset((played - thumb / 2).coerceIn(0f, size.width - thumb), (size.height - thumb) / 2), Size(thumb, thumb))
                }
            },
    )
}

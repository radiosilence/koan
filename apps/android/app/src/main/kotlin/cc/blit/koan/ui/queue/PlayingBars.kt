package cc.blit.koan.ui.queue

import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleStartEffect
import cc.blit.koan.state.PlayingLevels
import cc.blit.koan.theme.KoanTheme

/**
 * The playing indicator: three bars standing at the low, middle and high of
 * the spectrum. The levels are read inside `drawBehind` only, so a frame
 * invalidates this draw and recomposes nothing.
 *
 * With [live] false (paused) the bars stand flat and nothing is read from the
 * analyser. Attached from a *started* lifecycle and detached on stop: a
 * backgrounded app draws nothing, so it should not pay for an FFT.
 */
@Composable
fun PlayingBars(levels: PlayingLevels, live: Boolean, modifier: Modifier = Modifier) {
    val token = remember { Any() }
    val fps = LocalView.current.display?.refreshRate?.toInt()?.coerceAtMost(MAX_FPS) ?: MAX_FPS
    if (live) {
        LifecycleStartEffect(levels, fps) {
            levels.attach(token, fps)
            onStopOrDispose { levels.detach(token) }
        }
    }
    val colour = KoanTheme.colors.accent
    Box(
        modifier
            .size(12.dp, 11.dp)
            .drawBehind {
                val gap = 2.dp.toPx()
                val bar = (size.width - gap * 2) / 3
                val floor = 1.5.dp.toPx()
                for (i in 0 until 3) {
                    val level = if (live) levels.band(i) else 0f
                    val h = maxOf(floor, size.height * level)
                    drawRect(colour, Offset(i * (bar + gap), size.height - h), Size(bar, h))
                }
            },
    )
}

/** Capped below a high-refresh display's rate: three bars eleven points tall do not need 120. */
private const val MAX_FPS = 60

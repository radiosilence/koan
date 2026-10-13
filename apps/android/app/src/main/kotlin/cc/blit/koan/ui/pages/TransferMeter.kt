package cc.blit.koan.ui.pages

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.flowWithLifecycle
import cc.blit.koan.theme.KoanTheme
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import uniffi.koan_ffi.Transfer
import uniffi.koan_ffi.TransferFigure

/*
 * The idea of `TransferMeter.swift`, in Compose terms. The byte counts move
 * ten times a second, so nothing that holds a list of transfers collects them.
 * Only the leaf that draws a number does, and it does it in one of two ways:
 *
 *  - a bar follows the figure with an Animatable that is read in the DRAW
 *    phase, so a frame costs a redraw of one 4 dp rectangle and no composition
 *    at all, and the 10 Hz steps are glided over;
 *  - a line of text collects a STRING derived from the figure and
 *    distinct-until-changed, so it recomposes only when its words differ.
 *
 * A transfer that is not live (settled) reads no figures at all.
 */

/** The figure of one track, as a flow that emits only when that track's own numbers move. */
internal fun figureOf(figures: StateFlow<Map<Long, TransferFigure>>, trackId: Long, live: Boolean): Flow<TransferFigure?> =
    if (live) figures.map { it[trackId] }.distinctUntilChanged() else flowOf(null)

/** A download's bar: the whole length quiet, what has arrived lit. */
@Composable
fun TransferBar(
    transfer: Transfer,
    figures: StateFlow<Map<Long, TransferFigure>>,
    modifier: Modifier = Modifier,
) {
    val live = TransferText.isLive(transfer.state)
    val state = transfer.state
    val trackId = transfer.trackId
    val shown = remember(trackId) { Animatable(TransferText.fraction(state, if (live) figures.value[trackId] else null)) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(figures, trackId, state) {
        if (!live) {
            shown.snapTo(TransferText.fraction(state, null))
            return@LaunchedEffect
        }
        figureOf(figures, trackId, true).flowWithLifecycle(lifecycle).collect { figure ->
            val target = TransferText.fraction(state, figure)
            // A bar never goes backwards on a running transfer except to restart it.
            if (target < shown.value) shown.snapTo(target) else shown.animateTo(target, tween(METER_GLIDE_MS, easing = LinearEasing))
        }
    }
    FractionBar(modifier) { shown.value }
}

/** A bar for a fraction known only at draw time. `fraction` is read inside the draw lambda. */
@Composable
fun FractionBar(modifier: Modifier = Modifier, fraction: () -> Float) {
    val colors = KoanTheme.colors
    Canvas(modifier.fillMaxWidth().height(4.dp)) {
        // The quiet end is what has not arrived, the same way round as the seek bar.
        drawRect(colors.rule, size = size)
        val width = size.width * fraction().coerceIn(0f, 1f)
        if (width > 0f) drawRect(colors.ink, topLeft = Offset.Zero, size = Size(width, size.height))
    }
}

/**
 * A line of a download row's text that follows the fast figures. Recomposes
 * only when [text] gives different words, not when the numbers behind them move.
 */
@Composable
fun rememberTransferText(
    transfer: Transfer,
    figures: StateFlow<Map<Long, TransferFigure>>,
    text: (Transfer, TransferFigure?) -> String,
): State<String> {
    val live = TransferText.isLive(transfer.state)
    val flow = remember(transfer, figures, live) {
        figureOf(figures, transfer.trackId, live).map { text(transfer, it) }.distinctUntilChanged()
    }
    return flow.collectAsStateWithLifecycle(initialValue = text(transfer, if (live) figures.value[transfer.trackId] else null))
}

private const val METER_GLIDE_MS = 100

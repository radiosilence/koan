package cc.blit.koan.ui.settings

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.CacheDrawScope
import androidx.compose.ui.draw.DrawResult
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.ln
import kotlin.math.pow

/** A curve to draw: its levels at the response's frequencies, and how. */
@Immutable
data class Curve(val db: List<Double>, val color: Color, val width: Float = 1.5f, val dashed: Boolean = false)

/** The frequencies the graph marks, and how each is written. */
val hzTicks = listOf(20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0)

fun hzLabel(hz: Double) = if (hz >= 1000) "${(hz / 1000).toInt()}k" else "${hz.toInt()}"

/**
 * How far the graph reaches either side of 0 dB: the next 6 dB past
 * whatever is drawn, at least ±6, so a small correction is not drawn as a
 * large one.
 */
fun halfRange(curves: List<List<Double>>): Double {
    val reach = curves.flatten().maxOfOrNull { abs(it) } ?: 0.0
    return maxOf(6.0, ceil(reach / 6) * 6)
}

/** The step between dB gridlines: 10 dB over a wide range, else 6. */
fun dbStep(half: Double) = if (half * 2 > 30) 10.0 else 6.0

/** The dB gridlines from -`half` to `half`, 0 among them. */
fun dbTicks(half: Double): List<Double> {
    val step = dbStep(half)
    val n = (half / step).toInt()
    return (-n..n).map { it * step }
}

/** How many points a curve is drawn with, whatever the response holds: a pixel or two apart at a phone's width. */
const val POINTS = 256

/**
 * `db` at `freqs`, resampled to `count` points log-spaced from 20 Hz to
 * 20 kHz, each read off the curve linearly in log frequency.
 */
fun sample(freqs: List<Double>, db: List<Double>, count: Int = POINTS): List<Pair<Double, Double>> {
    val n = minOf(freqs.size, db.size)
    if (n == 0) return emptyList()
    val lo = ln(20.0)
    val hi = ln(20_000.0)
    var j = 0
    return (0 until count).map { i ->
        val hz = Math.E.pow(lo + (hi - lo) * i / (count - 1))
        while (j < n - 2 && freqs[j + 1] < hz) j++
        val f0 = freqs[j]
        val f1 = freqs[minOf(j + 1, n - 1)]
        val t = if (f1 > f0) ((ln(hz) - ln(f0)) / (ln(f1) - ln(f0))).coerceIn(0.0, 1.0) else 0.0
        hz to db[j] + t * (db[minOf(j + 1, n - 1)] - db[j])
    }
}

/** Colours and type the graph draws its frame in. */
@Immutable
data class GraphStyle(val grid: Color, val zero: Color, val label: Color, val text: TextStyle)

/**
 * A response drawn on a log-frequency axis from 20 Hz to 20 kHz, with
 * frequency and dB gridlines, their labels, and a heavier 0 dB line. The
 * curves are resampled to `POINTS` points and their paths built once for
 * each size and set of curves, not on each frame or recomposition;
 * `onBuild` is told each time they are.
 */
@Composable
fun ResponseGraph(
    freqs: List<Double>,
    curves: List<Curve>,
    style: GraphStyle,
    modifier: Modifier = Modifier,
    /** The dB range either side of 0, held still while a drag redraws the curves; drawn from the curves without one. */
    fixedHalf: Double? = null,
    onBuild: () -> Unit = {},
    overlay: @Composable () -> Unit = {},
) {
    val measurer = rememberTextMeasurer()
    // One lambda per set of curves: a new one would rebuild the cache.
    val build = remember(freqs, curves, style, measurer, fixedHalf) {
        val sampled = curves.map { sample(freqs, it.db) }
        val half = fixedHalf ?: halfRange(curves.map { it.db })
        val draw: CacheDrawScope.() -> DrawResult = {
            onBuild()
            graph(sampled, curves, half, style, measurer)
        }
        draw
    }
    Box(modifier) {
        Box(Modifier.fillMaxSize().drawWithCache(build))
        overlay()
    }
}

/** Room on the left of the plot for the dB labels, and below it for the frequencies. */
val LABEL_LEFT = 30.dp
val LABEL_BOTTOM = 16.dp

/**
 * The graph's scale: 20 Hz to 20 kHz across on a log scale after the labels'
 * room, ±`half` dB down to theirs. Turns both ways, for a drag on the graph;
 * a drag is held to what a band takes, 10 Hz–22 kHz and ±30 dB.
 */
class Plot(width: Float, height: Float, val half: Double, val left: Float = 0f, bottom: Float = 0f) {
    val right = width
    val top = 0f
    val foot = height - bottom
    private val lo = ln(20.0)
    private val hi = ln(20_000.0)

    fun x(hz: Double) = (left + (ln(hz.coerceIn(20.0, 20_000.0)) - lo) / (hi - lo) * (right - left)).toFloat()

    fun y(db: Double) = (top + (half - db.coerceIn(-half, half)) / (2 * half) * (foot - top)).toFloat()

    fun hz(x: Float) = Math.E.pow(lo + ((x - left) / (right - left)).coerceIn(0f, 1f) * (hi - lo)).coerceIn(10.0, 22_000.0)

    fun db(y: Float) = (half - (y - top) / (foot - top) * 2 * half).coerceIn(-30.0, 30.0)
}

private fun CacheDrawScope.graph(
    sampled: List<List<Pair<Double, Double>>>,
    curves: List<Curve>,
    half: Double,
    style: GraphStyle,
    measurer: TextMeasurer,
): DrawResult {
    val hzText = hzTicks.map { measurer.measure(hzLabel(it), style.text) }
    val dbText = dbTicks(half).map { db -> measurer.measure(if (db > 0) "+${db.toInt()}" else "${db.toInt()}", style.text) }
    val plot = Plot(size.width, size.height, half, LABEL_LEFT.toPx(), LABEL_BOTTOM.toPx())
    val paths = sampled.map { points ->
        Path().apply {
            points.forEachIndexed { i, (hz, db) -> if (i == 0) moveTo(plot.x(hz), plot.y(db)) else lineTo(plot.x(hz), plot.y(db)) }
        }
    }
    val strokes = curves.map { c ->
        Stroke(c.width.dp.toPx(), pathEffect = if (c.dashed) PathEffect.dashPathEffect(floatArrayOf(6.dp.toPx(), 4.dp.toPx())) else null)
    }
    val hair = 1.dp.toPx()
    return onDrawBehind {
        hzTicks.forEachIndexed { i, hz ->
            val x = plot.x(hz)
            drawLine(style.grid, Offset(x, plot.top), Offset(x, plot.foot), hair)
            val text = hzText[i]
            val at = (x - text.size.width / 2f).coerceIn(plot.left, plot.right - text.size.width)
            drawText(text, style.label, Offset(at, plot.foot + 2.dp.toPx()))
        }
        dbTicks(half).forEachIndexed { i, db ->
            val y = plot.y(db)
            drawLine(if (db == 0.0) style.zero else style.grid, Offset(plot.left, y), Offset(plot.right, y), if (db == 0.0) hair * 1.5f else hair)
            val text = dbText[i]
            val at = (y - text.size.height / 2f).coerceIn(plot.top, plot.foot - text.size.height)
            drawText(text, style.label, Offset(0f, at))
        }
        paths.forEachIndexed { i, path -> drawPath(path, curves[i].color, style = strokes[i]) }
    }
}

/** The graph's frame in the theme: gridlines in `rule`, labels in `fine`, `muted`, 0 dB in `muted`. */
@Composable
fun graphStyle() = GraphStyle(
    grid = KoanTheme.color(KoanTone.Rule),
    zero = KoanTheme.color(KoanTone.Muted),
    label = KoanTheme.color(KoanTone.Muted),
    text = KoanType.Fine.style,
)


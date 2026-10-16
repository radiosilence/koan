package cc.blit.koan.ui.settings

import cc.blit.koan.ffi.DspBand
import cc.blit.koan.ffi.DspPoint
import cc.blit.koan.ffi.DspTuningEntry
import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.ln
import kotlin.math.roundToLong
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/** A band type that can be chosen, by the name the config uses: the short form a row shows and the name a menu gives. */
data class BandKind(val id: String, val short: String, val name: String)

val bandKinds = listOf(
    BandKind("peaking", "PK", "Peak"),
    BandKind("low_shelf", "LS", "Low shelf"),
    BandKind("high_shelf", "HS", "High shelf"),
    BandKind("low_pass", "LP", "Low pass"),
    BandKind("high_pass", "HP", "High pass"),
    BandKind("notch", "NO", "Notch"),
    BandKind("band_pass", "BP", "Band pass"),
    BandKind("all_pass", "AP", "All pass"),
)

/** Whether a band's figures can be typed into: a parametric band, not a graphic curve, delay or mix. */
fun editable(kind: String) = bandKinds.any { it.id == kind }

/** A band the graph draws a handle for: one with a gain to drag. */
data class Handle(val index: Int, val hz: Double, val db: Double, val q: Double)

/** The handles for `bands` on `channel`: peaks and shelves, each with its index among all the filters. */
fun handles(bands: List<DspBand>, channel: UShort = 0u) = bands.mapIndexedNotNull { i, b ->
    if (b.kind in setOf("peaking", "low_shelf", "high_shelf") && (b.channels.isEmpty() || channel in b.channels)) {
        Handle(i, b.freq, b.gainDb, b.q)
    } else {
        null
    }
}

/** The graphic curve a drag away from the handles paints: the first one, on the left channel. */
fun curveIndex(bands: List<DspBand>) = bands.indexOfFirst { it.kind == "graphic" && (it.channels.isEmpty() || 0.toUShort() in it.channels) }
    .takeIf { it >= 0 }

/** How far a brush reaches: the nearest point alone, a third of an octave, or an octave. */
enum class Brush(val label: String, val octaves: Double) { Point("Point", 0.0), Narrow("Narrow", 1.0 / 3), Wide("Wide", 1.0) }

/**
 * The curve pulled toward `db` at `hz`: the nearest point alone, or its
 * neighbours too, less the further they are. Points the brush barely reaches
 * keep what they had, and what it paints is kept to a hundredth of a dB.
 */
fun paint(points: List<DspPoint>, hz: Double, db: Double, brush: Brush): List<DspPoint> {
    fun octaves(p: DspPoint) = abs(ln(p.hz / hz) / ln(2.0))
    if (brush == Brush.Point) {
        val i = points.indices.minByOrNull { octaves(points[it]) } ?: return points
        return points.mapIndexed { j, p -> if (j == i) DspPoint(p.hz, db) else p }
    }
    return points.map { p ->
        val d = octaves(p) / brush.octaves
        val reach = exp(-d * d)
        if (reach <= 0.01) p else DspPoint(p.hz, ((p.db + reach * (db - p.db)) * 100).roundToLong() / 100.0)
    }
}

// -- A tuning chain: the EQs an output plays on top of its correction, in order.

fun List<DspTuningEntry>.moved(i: Int, by: Int): List<DspTuningEntry> {
    val to = i + by
    if (i !in indices || to !in indices) return this
    return toMutableList().apply { add(to, removeAt(i)) }
}

fun List<DspTuningEntry>.toggled(i: Int) = mapIndexed { j, e -> if (j == i) DspTuningEntry(e.name, !e.on) else e }

fun List<DspTuningEntry>.removed(i: Int) = filterIndexed { j, _ -> j != i }

/** `name` on the end, playing; nothing changes if it is in the chain already. */
fun List<DspTuningEntry>.added(name: String) = if (any { it.name == name }) this else this + DspTuningEntry(name, true)

/**
 * An edit dragged on the graph, step by step: each step drawn at once with a
 * preview, one at a time and always the newest, and saved (and so played) at
 * most every quarter second while it goes on and once at its end. The first
 * step waits too: a touch that was the start of a scroll saves nothing.
 */
class EditStepper<R>(
    private val scope: CoroutineScope,
    private val now: () -> Long,
    private val show: (R) -> Unit,
) {
    var editing = false
        private set
    private var saved = 0L
    private var unsaved: (() -> Unit)? = null
    private var drawing = false
    private var waiting: (suspend () -> R?)? = null

    fun step(done: Boolean, preview: suspend () -> R?, save: () -> Unit) {
        if (!editing) saved = now()
        editing = !done
        if (done || now() - saved > THROTTLE_MS) {
            save()
            saved = now()
            unsaved = null
        } else {
            unsaved = save
        }
        when {
            done -> waiting = null
            drawing -> waiting = preview
            else -> draw(preview)
        }
    }

    /** The page is going: save what the throttle held back. */
    fun flush() {
        unsaved?.invoke()
        unsaved = null
        editing = false
        waiting = null
    }

    private fun draw(preview: suspend () -> R?) {
        drawing = true
        scope.launch {
            val drawn = preview()
            if (editing && drawn != null) show(drawn)
            val next = waiting
            waiting = null
            if (next != null) draw(next) else drawing = false
        }
    }

    companion object {
        const val THROTTLE_MS = 250L
    }
}

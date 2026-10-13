package cc.blit.koan.state

import androidx.compose.runtime.mutableFloatStateOf
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import uniffi.koan_ffi.VizStreamInterface
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow

/**
 * The audio behind the playing indicators. The Kotlin port of
 * `PlayingLevels.swift`; its doc comments are the specification.
 *
 * One subscription for the whole app: the queue and a track list can both
 * have a current row on screen, and they are watching the same music. Nothing
 * here is polled and nothing is timed: the analyser publishes a frame and this
 * wakes on it. When the playhead stops and the bars have fallen the analyser
 * publishes nothing, so the loop is asleep and costs nothing until there is
 * music again.
 *
 * Nothing here is observed by composition either. The three bands are
 * snapshot state read only inside a `drawBehind` block, so a frame invalidates
 * the draw of the bars showing it and never recomposes anything.
 *
 * The stream is read **only while an indicator is attached**. With none on
 * screen it is not opened at all, and the analyser, which parks when nothing
 * reads it, parks. Attach from a started lifecycle and detach on stop, so a
 * backgrounded app (music still playing, bars attached to views nobody can
 * see) does not run an FFT at the display's rate.
 *
 * [scope] runs on the engine dispatcher: opening the stream and each `next()`
 * cross the FFI and do not belong on Main. [attach] and [detach] are cheap and
 * callable from any thread.
 */
class PlayingLevels(
    private val scope: CoroutineScope,
    private val open: () -> VizStreamInterface,
    private val setFps: (Int) -> Unit,
    private val nanoClock: () -> Long = System::nanoTime,
) {
    private val bands = arrayOf(mutableFloatStateOf(0f), mutableFloatStateOf(0f), mutableFloatStateOf(0f))
    private val ceiling = doubleArrayOf(QUIETEST, QUIETEST, QUIETEST)
    private var stamp = nanoClock()

    private val lock = Any()
    private val attached = LinkedHashSet<Any>()
    private var follow: Job? = null

    /** How high a bar stands, 0 to 1: band 0 is the low end, 2 the high. Silence is zero and reads flat. */
    fun band(index: Int): Float = bands[index].floatValue

    /** How many indicators are listening. */
    val attachedCount: Int get() = synchronized(lock) { attached.size }

    /** Whether the stream is open, which is exactly while at least one indicator is attached. */
    val following: Boolean get() = synchronized(lock) { follow != null }

    /**
     * An indicator that wants the music, under a token of its own. The first
     * one opens the stream. `fps` is the refresh rate of the display it is
     * drawn on, which only the window knows; the analyser runs at that.
     */
    fun attach(token: Any, fps: Int) {
        synchronized(lock) {
            attached += token
            if (follow == null) follow = scope.launch { run() }
        }
        scope.launch { setFps(fps.coerceIn(1, 255)) }
    }

    /** An indicator that has stopped listening. The last one to leave ends the follow, and the analyser parks. */
    fun detach(token: Any) {
        val ended = synchronized(lock) {
            attached -= token
            if (attached.isEmpty()) follow.also { follow = null } else null
        }
        ended?.cancel()
    }

    private suspend fun run() {
        val stream = open()
        try {
            while (true) {
                val levels = stream.next() ?: break
                take(levels.low.toDouble(), levels.mid.toDouble(), levels.high.toDouble())
            }
        } finally {
            (stream as? AutoCloseable)?.close()
            // Nothing is reading: the bars rest, rather than stand where the
            // last frame left them until the next one.
            for (band in bands) band.floatValue = 0f
        }
    }

    /**
     * A frame, as it arrives. The only smoothing here is the ceiling each band
     * is measured against; the fall is the analyser's, which is also what
     * decays the bars to flat when the music stops.
     *
     * Each band is judged against its own recent range rather than full scale,
     * which is what stops a track mastered quiet getting a limper indicator
     * than a loud one, and incidentally undoes the analyser's A-weighting
     * tilt, which otherwise leaves the bass bar permanently the sluggish one.
     */
    internal fun take(low: Double, mid: Double, high: Double) {
        val now = nanoClock()
        // Clamped: a phone that slept owes the bars nothing.
        val elapsed = ((now - stamp) / 1e9).coerceIn(0.0, 0.25)
        stamp = now
        val hold = 0.5.pow(elapsed / FORGET_SECONDS)
        val heard = doubleArrayOf(low, mid, high)
        for (i in heard.indices) {
            ceiling[i] = max(heard[i], max(ceiling[i] * hold, QUIETEST))
            val next = min(heard[i] / ceiling[i], 1.0).toFloat()
            // A frame that says what the last one said moves nothing.
            if (bands[i].floatValue != next) bands[i].floatValue = next
        }
    }

    companion object {
        /**
         * A band below this is room tone, and never sets a ceiling. It is also
         * what a silent passage is measured against, so silence stays flat
         * rather than being normalised back up into a dance.
         */
        const val QUIETEST = 0.12

        /** How long a band takes to forget a loud passage. */
        const val FORGET_SECONDS = 4.0
    }
}

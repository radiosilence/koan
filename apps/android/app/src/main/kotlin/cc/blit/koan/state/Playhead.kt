package cc.blit.koan.state

/**
 * Where the playhead was, and whether it is still moving.
 *
 * The engine publishes one when a client's own reckoning would go wrong (a
 * seek, a pause, a track boundary, a stall), so everything that shows a
 * position derives it from here rather than waiting to be told a number.
 * Nothing ticks to keep a position up to date; the anchor is what makes that
 * possible.
 */
data class Playhead(
    val positionMs: Long,
    val playing: Boolean,
    /** [System.nanoTime] when this arrived. Local: compared only with the same clock. */
    val atNanos: Long,
) {
    /** Where the playhead is at `nowNanos`, by this anchor's reckoning. */
    fun at(nowNanos: Long = System.nanoTime()): Long {
        if (!playing) return positionMs
        return positionMs + maxOf(0L, nowNanos - atNanos) / 1_000_000L
    }

    /**
     * The same, capped at a track's length: a prediction runs past the end of
     * a track for as long as it takes the engine to say the next one started.
     */
    fun at(nowNanos: Long = System.nanoTime(), withinMs: Long): Long =
        if (withinMs > 0) minOf(at(nowNanos), withinMs) else at(nowNanos)

    companion object {
        val Stopped = Playhead(0, false, 0)
    }
}

/** What koan is doing on its own, as one value: the only thing that reads either draws a row for each. */
data class EngineTasks(val scanning: Boolean, val syncing: Boolean)

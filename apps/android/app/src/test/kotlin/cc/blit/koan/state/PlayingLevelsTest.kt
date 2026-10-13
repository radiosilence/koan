package cc.blit.koan.state

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.VizLevels
import uniffi.koan_ffi.VizStreamInterface

/** A stream fed by hand that records whether it was opened and closed. */
private class FakeViz : VizStreamInterface, AutoCloseable {
    val frames = Channel<VizLevels?>(Channel.UNLIMITED)
    var closed = false
    override suspend fun next(): VizLevels? = frames.receive()
    override fun close() { closed = true }
}

@OptIn(ExperimentalCoroutinesApi::class)
class PlayingLevelsTest {
    private var nowNanos = 0L
    private var opened = 0
    private val fps = mutableListOf<Int>()
    private val streams = mutableListOf<FakeViz>()

    private fun TestScope.levels() = PlayingLevels(
        backgroundScope, open = { opened++; FakeViz().also { streams += it } }, setFps = { fps += it },
        nanoClock = { nowNanos },
    )

    private fun frame(low: Float, mid: Float = 0f, high: Float = 0f) = VizLevels(low, mid, high)

    @Test
    fun `nothing is opened until an indicator attaches`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        runCurrent()
        assertEquals(0, opened)
        assertFalse(levels.following)
    }

    @Test
    fun `many indicators share one stream, opened by the first`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        val a = Any(); val b = Any()
        levels.attach(a, 60)
        levels.attach(b, 60)
        runCurrent()
        assertEquals(1, opened)
        assertEquals(2, levels.attachedCount)
        assertTrue(levels.following)
    }

    @Test
    fun `the last indicator to leave closes the stream and the bars rest`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        val a = Any(); val b = Any()
        levels.attach(a, 60)
        levels.attach(b, 60)
        runCurrent()
        streams[0].frames.trySend(frame(1f, 1f, 1f))
        runCurrent()
        assertEquals(1f, levels.band(0), 0.001f)

        levels.detach(a)
        runCurrent()
        assertTrue("one still listening", levels.following)
        assertFalse(streams[0].closed)

        levels.detach(b)
        runCurrent()
        assertFalse(levels.following)
        assertTrue("with nobody reading, the stream is let go so the analyser parks", streams[0].closed)
        assertEquals(0f, levels.band(0), 0f)
    }

    @Test
    fun `attaching again after a full detach opens a fresh stream`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        val a = Any()
        levels.attach(a, 60); runCurrent()
        levels.detach(a); runCurrent()
        levels.attach(a, 60); runCurrent()
        assertEquals(2, opened)
    }

    @Test
    fun `detaching something never attached changes nothing`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        levels.detach(Any())
        assertEquals(0, levels.attachedCount)
        assertEquals(0, opened)
    }

    @Test
    fun `the display's rate is passed to the analyser, within what it accepts`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        levels.attach(Any(), 90); runCurrent()
        levels.attach(Any(), 0); runCurrent()
        assertEquals(listOf(90, 1), fps)
    }

    @Test
    fun `a stream that ends ends the follow and rests the bars`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        levels.attach(Any(), 60); runCurrent()
        streams[0].frames.trySend(frame(0.5f))
        streams[0].frames.trySend(null)
        runCurrent()
        assertTrue(streams[0].closed)
        assertEquals(0f, levels.band(0), 0f)
    }

    @Test
    fun `each band is judged against its own recent loudest`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        levels.take(0.5, 0.25, 0.0)
        assertEquals(1.0f, levels.band(0), 0.001f)
        assertEquals(1.0f, levels.band(1), 0.001f)
        assertEquals(0.0f, levels.band(2), 0.001f)
        // A quieter frame against the same ceiling stands lower.
        levels.take(0.25, 0.25, 0.0)
        assertEquals(0.5f, levels.band(0), 0.001f)
    }

    @Test
    fun `room tone never sets a ceiling, so silence stays flat`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        levels.take(0.06, 0.06, 0.06)
        assertEquals(0.5f, levels.band(0), 0.001f)
        levels.take(0.0, 0.0, 0.0)
        assertEquals(0f, levels.band(0), 0f)
    }

    @Test
    fun `the ceiling is forgotten over seconds, not frames`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        levels.take(1.0, 0.0, 0.0)
        // Four seconds of 0.25 s frames halves the ceiling from 1.0 to 0.5.
        repeat(16) { nowNanos += 250_000_000; levels.take(0.25, 0.0, 0.0) }
        assertEquals(0.5f, levels.band(0), 0.01f)
    }

    @Test
    fun `a long gap owes the bars nothing`() = runTest(UnconfinedTestDispatcher()) {
        val levels = levels()
        levels.take(1.0, 0.0, 0.0)
        nowNanos += 3_600_000_000_000 // a phone that slept an hour
        levels.take(0.5, 0.0, 0.0)
        // Forgotten by at most the clamped quarter second, not by an hour.
        assertEquals(0.5f / 1.0f, levels.band(0), 0.05f)
    }
}

package cc.blit.koan.state

import cc.blit.koan.FakeEngine
import cc.blit.koan.FakeStream
import cc.blit.koan.item
import cc.blit.koan.nowPlaying
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.KoanException
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.RepeatMode
import uniffi.koan_ffi.StateSlice

@OptIn(ExperimentalCoroutinesApi::class)
class PlayerModelTest {
    private var nowNanos = 0L
    private val mirror = EngineMirror(nanoClock = { nowNanos })

    private fun TestScope.model(engine: FakeEngine) =
        PlayerModel(engine.engine, mirror, backgroundScope, nanoClock = { nowNanos })

    private fun TestScope.advance(ms: Long) {
        nowNanos += ms * 1_000_000
        advanceTimeBy(ms)
        runCurrent()
    }

    @Test
    fun `transport commands reach the engine in the order they were asked`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        model.resume()
        model.pause()
        model.next()
        model.previous()
        model.stop()
        runCurrent()
        assertEquals(listOf("resume", "pause", "next", "previous", "stop"), engine.calls)
    }

    @Test
    fun `modes cycle and are sent as the engine names them`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        mirror.apply(StateSlice.Playback(nowPlaying(shuffle = false, repeat = RepeatMode.OFF)))
        model.toggleShuffle()
        model.cycleRepeat()
        runCurrent()
        assertEquals(listOf("setShuffle(true)", "setRepeat(QUEUE)"), engine.calls)
        mirror.apply(StateSlice.Playback(nowPlaying(repeat = RepeatMode.QUEUE)))
        model.cycleRepeat()
        mirror.apply(StateSlice.Playback(nowPlaying(repeat = RepeatMode.ONE)))
        model.cycleRepeat()
        runCurrent()
        assertEquals(listOf("setRepeat(ONE)", "setRepeat(OFF)"), engine.calls.drop(2))
    }

    @Test
    fun `a failing command becomes the error banner and not a crash`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "next") throw KoanException.Audio("no output") else Unit }
        val model = model(engine)
        model.next()
        runCurrent()
        assertNotNull(model.lastError.value)
        model.dismissError()
        assertNull(model.lastError.value)
    }

    @Test
    fun `a play still resolving ignores play and pause until the queue is in`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        val job = model.playNow("Album") { listOf(1L, 2L) }
        assertEquals("Album", model.resolving.value)
        assertEquals(1, model.pendingMutations.value)
        model.togglePlayPause()
        model.resume()
        runCurrent()
        job.join()
        assertNull(model.resolving.value)
        assertEquals(0, model.pendingMutations.value)
        assertEquals(listOf("replaceQueue([1, 2], 0, false)"), engine.calls)
    }

    @Test
    fun `a listing longer than the window is cut at the track tapped`() {
        val ids = (1L..1200L).toList()
        assertEquals(ids.take(400) to 5, PlayerModel.listing(ids.take(400), 5))
        val (queued, start) = PlayerModel.listing(ids, 700)
        assertEquals(PlayerModel.LISTING_WINDOW, queued.size)
        assertEquals(701L, queued.first())
        assertEquals(0, start)
        assertEquals(ids to 0, PlayerModel.listing(ids, null))
        assertEquals(0, PlayerModel.listing(ids.take(10), 99).second)
    }

    @Test
    fun `play next with nothing playing appends`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        model.playNext(listOf(4L))
        runCurrent()
        assertEquals(listOf("addToQueue([4])"), engine.calls)
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, item("cur", 9))))
        model.playNext(listOf(5L))
        runCurrent()
        assertEquals("insertAfter([5], cur)", engine.calls.last())
    }

    @Test
    fun `a seek holds the thumb until the engine catches up`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, item("a", 1), durationMs = 100_000uL)))
        mirror.apply(StateSlice.Playhead(10_000uL, 100_000uL, playing = false))
        model.start()
        runCurrent()

        model.seekTo(60_000)
        runCurrent()
        assertEquals(0.6, model.scrubbing.value!!, 1e-9)
        assertEquals("seek(60000)", engine.calls.last())
        assertEquals(0.6, model.progress(), 1e-9)

        // The engine's own, old position must not hand the bar back.
        mirror.apply(StateSlice.Playhead(10_500uL, 100_000uL, playing = false))
        runCurrent()
        assertNotNull(model.scrubbing.value)

        // Within tolerance of the target: released.
        mirror.apply(StateSlice.Playhead(60_200uL, 100_000uL, playing = false))
        runCurrent()
        assertNull(model.scrubbing.value)
    }

    @Test
    fun `a seek the engine never honours gives the bar back after the patience`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, item("a", 1), durationMs = 100_000uL)))
        mirror.apply(StateSlice.Playhead(10_000uL, 100_000uL, playing = false))
        model.seekTo(60_000)
        runCurrent()
        assertNotNull(model.scrubbing.value)
        advance(PlayerModel.SEEK_PATIENCE_MS + 1)
        assertNull(model.scrubbing.value)
    }

    @Test
    fun `a seek is clamped to what has arrived and refused before anything has`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, item("a", 1), durationMs = 100_000uL)))
        // Nothing seekable yet: a download that cannot say what it is.
        model.seekTo(50_000)
        runCurrent()
        assertTrue(engine.calls.isEmpty())
        assertNotNull(model.lastNotice.value)

        mirror.apply(StateSlice.Playhead(0uL, 30_000uL, playing = false))
        assertEquals(0.3, model.seekable, 1e-9)
        model.seekTo(50_000)
        runCurrent()
        assertEquals("seek(30000)", engine.calls.last())
    }

    @Test
    fun `scrubbing takes the bar from a seek still settling`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, item("a", 1), durationMs = 100_000uL)))
        mirror.apply(StateSlice.Playhead(0uL, 100_000uL, playing = false))
        model.seekTo(50_000)
        model.beginScrub(0.9)
        advance(PlayerModel.SEEK_PATIENCE_MS + 1)
        assertEquals(0.9, model.scrubbing.value!!, 1e-9)
    }

    @Test
    fun `where the playing track lives is looked up once per track, not per tick`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        model.start()
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, item("a", 7))))
        runCurrent()
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PAUSED, item("a", 7))))
        runCurrent()
        assertEquals(1, engine.calls.count { it.startsWith("track") })
        assertEquals(7L, model.place.value?.trackId)
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.STOPPED, null)))
        runCurrent()
        assertNull(model.place.value)
    }

    @Test
    fun `the session is saved on pause and as the queue changes while playing`() = runTest {
        val engine = FakeEngine()
        val model = model(engine)
        model.start()
        mirror.apply(StateSlice.Queue(listOf(item("a", 1)), 3uL))
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, item("a", 1))))
        mirror.apply(StateSlice.Playhead(0uL, 1uL, playing = true))
        runCurrent()
        advance(1_100)
        assertTrue(engine.calls.contains("saveSession"))
        // Unchanged queue: only the position from then on.
        advance(1_000)
        assertTrue(engine.calls.contains("savePosition"))
        val before = engine.calls.size
        mirror.apply(StateSlice.Playhead(5_000uL, 1uL, playing = false))
        runCurrent()
        assertTrue(engine.calls.size > before)
    }

    @Test
    fun `restore waits for the mirror to hold the queue it was told about`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "restoreSession") 2u else Unit }
        val model = model(engine)
        val stream = FakeStream()
        val follower = launch { mirror.follow(stream) }
        val restored = async { model.restoreSession(); true }
        runCurrent()
        stream.send(StateSlice.Queue(listOf(item("a", 1), item("b", 2)), 1uL))
        runCurrent()
        assertTrue(restored.isCompleted)
        stream.close()
        follower.join()
    }
}

package cc.blit.koan.state

import cc.blit.koan.FakeEngine
import cc.blit.koan.item
import cc.blit.koan.nowPlaying
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.LyricLine
import uniffi.koan_ffi.Lyrics
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.StateSlice

private fun lyrics(synced: Boolean, source: String = "lrclib") = Lyrics(
    content = "words", synced = synced, source = source,
    lines = if (synced) listOf(LyricLine(1.0, "one"), LyricLine(5.0, "two"), LyricLine(9.5, "three")) else emptyList(),
)

@OptIn(ExperimentalCoroutinesApi::class)
class LyricsModelTest {
    private val mirror = EngineMirror()

    private fun play(track: Long?) = mirror.apply(
        StateSlice.Playback(nowPlaying(PlayState.PLAYING, entry = track?.let { item("q$it", track = it) })),
    )

    @Test
    fun `a cached synced copy fills the panel and LRCLIB is not asked`() = runTest(UnconfinedTestDispatcher()) {
        val engine = FakeEngine { name, _ -> if (name == "lyrics") lyrics(true, "cache") else error("asked $name") }
        val model = LyricsModel(engine.engine, mirror, UnconfinedTestDispatcher(testScheduler))
        play(7)
        val job = launch { model.follow() }
        runCurrent()
        assertEquals(7L, model.state.value.trackId)
        assertEquals("cache", model.state.value.lyrics?.source)
        assertFalse(model.state.value.loading)
        assertEquals(listOf("lyrics(7)"), engine.calls)
        job.cancel()
    }

    @Test
    fun `a plain copy is shown at once, then replaced by what LRCLIB has`() = runTest(UnconfinedTestDispatcher()) {
        val engine = FakeEngine { name, _ ->
            when (name) {
                "lyrics" -> lyrics(false, "cache")
                "fetchLyrics" -> lyrics(true, "lrclib")
                else -> null
            }
        }
        val model = LyricsModel(engine.engine, mirror, UnconfinedTestDispatcher(testScheduler))
        play(7)
        val job = launch { model.follow() }
        runCurrent()
        assertEquals(listOf("lyrics(7)", "fetchLyrics(7)"), engine.calls)
        assertTrue(model.state.value.lyrics!!.synced)
        assertFalse(model.state.value.loading)
        job.cancel()
    }

    @Test
    fun `a miss everywhere is the normal case, not an error`() = runTest(UnconfinedTestDispatcher()) {
        val engine = FakeEngine()
        val model = LyricsModel(engine.engine, mirror, UnconfinedTestDispatcher(testScheduler))
        play(7)
        val job = launch { model.follow() }
        runCurrent()
        assertNull(model.state.value.lyrics)
        assertEquals(7L, model.state.value.trackId)
        assertFalse(model.state.value.loading)
        job.cancel()
    }

    @Test
    fun `a failing fetch keeps the cached copy`() = runTest(UnconfinedTestDispatcher()) {
        val engine = FakeEngine { name, _ -> if (name == "lyrics") lyrics(false, "cache") else throw IllegalStateException("offline") }
        val model = LyricsModel(engine.engine, mirror, UnconfinedTestDispatcher(testScheduler))
        play(7)
        val job = launch { model.follow() }
        runCurrent()
        assertEquals("cache", model.state.value.lyrics?.source)
        job.cancel()
    }

    @Test
    fun `the words follow the track, and nothing is playing means none`() = runTest(UnconfinedTestDispatcher()) {
        val engine = FakeEngine { name, args -> if (name == "lyrics") lyrics(true, "for ${args[0]}") else null }
        val model = LyricsModel(engine.engine, mirror, UnconfinedTestDispatcher(testScheduler))
        play(1)
        val job = launch { model.follow() }
        runCurrent()
        assertEquals("for 1", model.state.value.lyrics?.source)
        play(2)
        runCurrent()
        assertEquals("for 2", model.state.value.lyrics?.source)
        assertEquals(2L, model.state.value.trackId)
        play(null)
        runCurrent()
        assertEquals(LyricsState.None, model.state.value)
        job.cancel()
    }

    @Test
    fun `nothing is asked until the panel follows`() = runTest(UnconfinedTestDispatcher()) {
        val engine = FakeEngine()
        LyricsModel(engine.engine, mirror, UnconfinedTestDispatcher(testScheduler))
        play(7)
        runCurrent()
        assertEquals(emptyList<String>(), engine.calls)
    }

    // MARK: - Timeline

    private val lines = lyrics(true).lines

    @Test
    fun `the current line is the last whose timestamp has passed`() {
        assertEquals(-1, LyricsTimeline.currentIndex(lines, 0.5))
        assertEquals(0, LyricsTimeline.currentIndex(lines, 1.0))
        assertEquals(0, LyricsTimeline.currentIndex(lines, 4.99))
        assertEquals(1, LyricsTimeline.currentIndex(lines, 5.0))
        assertEquals(2, LyricsTimeline.currentIndex(lines, 100.0))
        assertEquals(-1, LyricsTimeline.currentIndex(emptyList(), 3.0))
    }

    @Test
    fun `the wait is the distance to the next timestamp, and none after the last`() {
        assertEquals(500L + 1, LyricsTimeline.millisToNext(lines, 0.5))
        assertEquals(4000L + 1, LyricsTimeline.millisToNext(lines, 1.0))
        assertNull(LyricsTimeline.millisToNext(lines, 9.5))
        assertNull(LyricsTimeline.millisToNext(emptyList(), 0.0))
    }
}

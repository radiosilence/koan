package cc.blit.koan.engine

import cc.blit.koan.FakeEngine
import cc.blit.koan.art.ArtworkBytes
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.art.ArtworkStore
import cc.blit.koan.art.CoverBytes
import cc.blit.koan.item
import cc.blit.koan.nowPlaying
import cc.blit.koan.state.ActivityModel
import cc.blit.koan.state.EngineMirror
import cc.blit.koan.state.LibraryModel
import cc.blit.koan.state.PlaylistsModel
import cc.blit.koan.theme.KoanAccent
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.StateSlice
import uniffi.koan_ffi.SyncPhase
import uniffi.koan_ffi.SyncProgress

@OptIn(ExperimentalCoroutinesApi::class)
class ModelWiringTest {
    private val mirror = EngineMirror()
    private val engine = FakeEngine { name, _ -> if (name == "playlists") emptyList<Any>() else null }
    private var sleeve: Int? = 0xCC3333
    private val covers = CoverBytes { _, _ -> ArtworkBytes(byteArrayOf(1), "image/png") }
    private val activity = ActivityModel()

    /** Calls with no arguments are recorded by bare name. */
    private fun FakeEngine.count(name: String) = synchronized(calls) { calls.count { it == name } }

    private fun TestScope.wiring(): ModelWiring {
        val art = ArtworkStore(covers, { sleeve }, StandardTestDispatcher(testScheduler), backgroundScope)
        val library = LibraryModel(engine.engine, mirror, backgroundScope, art = art)
        val playlists = PlaylistsModel(engine.engine, mirror, backgroundScope)
        return ModelWiring(backgroundScope, mirror, library, playlists, activity, art).also { it.start() }
    }

    @Test
    fun `the library is read at start and again whenever its rows move`() = runTest {
        wiring()
        runCurrent()
        assertEquals(1, engine.count("playlists"))
        assertEquals(1, engine.count("libraryStats"))
        mirror.apply(StateSlice.Library(1uL))
        runCurrent()
        assertEquals(2, engine.count("playlists"))
        assertEquals(2, engine.count("libraryStats"))
        // The same version again is not a change.
        mirror.apply(StateSlice.Library(1uL))
        runCurrent()
        assertEquals(2, engine.count("playlists"))
    }

    @Test
    fun `the engine's own scan and sync show as rows, and end with the work`() = runTest {
        wiring()
        runCurrent()
        mirror.apply(StateSlice.Tasks(scanning = true, syncing = true))
        runCurrent()
        assertEquals(listOf("Scanning library", "Syncing with server"), activity.tasks.value.map { it.label })
        mirror.apply(StateSlice.Sync(SyncProgress(SyncPhase.TRACKS, 10uL, 100uL)))
        runCurrent()
        assertEquals(0.1, activity.tasks.value.single { it.followsSync }.progress!!, 1e-9)
        mirror.apply(StateSlice.Tasks(scanning = false, syncing = false))
        runCurrent()
        assertEquals(emptyList<Any>(), activity.tasks.value)
    }

    @Test
    fun `the record playing sets the accent, and nothing playing is mint`() = runTest {
        val wiring = wiring()
        runCurrent()
        assertEquals(KoanAccent.Mint, wiring.accent.value)
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, entry = item("a", track = 5))))
        runCurrent()
        assertEquals(KoanAccent.of(0xCC3333), wiring.accent.value)
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.STOPPED)))
        runCurrent()
        assertEquals(KoanAccent.Mint, wiring.accent.value)
        sleeve = null
        mirror.apply(StateSlice.Playback(nowPlaying(PlayState.PLAYING, entry = item("b", track = 6))))
        runCurrent()
        assertEquals("a sleeve with no colour in it", KoanAccent.Mint, wiring.accent.value)
        assertTrue(ArtworkSource.Track(5).key.isNotEmpty())
    }
}

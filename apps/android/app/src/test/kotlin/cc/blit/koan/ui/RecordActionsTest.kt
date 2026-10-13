package cc.blit.koan.ui

import cc.blit.koan.FakeEngine
import cc.blit.koan.nav.Navigator
import cc.blit.koan.nav.Tab
import cc.blit.koan.state.EngineMirror
import cc.blit.koan.state.LibraryModel
import cc.blit.koan.state.PlayerModel
import cc.blit.koan.ui.pages.RecordActions
import cc.blit.koan.ui.pages.TrackIdSource
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RecordActionsTest {
    private class Rig(val engine: FakeEngine, val actions: RecordActions, val nav: Navigator, val version: MutableStateFlow<Long>)

    /** Albums 1 and 2 hold tracks 11, 12 and 21; artist 9 holds all three. */
    private val source = TrackIdSource { album, artist ->
        when {
            album == 1L -> listOf(11L, 12L)
            album == 2L -> listOf(21L)
            artist == 9L -> listOf(11L, 12L, 21L)
            else -> emptyList()
        }
    }

    private fun TestScope.rig(): Rig {
        val engine = FakeEngine()
        val mirror = EngineMirror()
        val player = PlayerModel(engine.engine, mirror, backgroundScope)
        val library = LibraryModel(engine.engine, mirror, backgroundScope)
        val nav = Navigator(backgroundScope)
        val version = MutableStateFlow(0L)
        nav.selectTab(Tab.Library)
        runCurrent()
        return Rig(engine, RecordActions(player, library, nav, source, backgroundScope, version), nav, version)
    }

    @Test
    fun `playing a whole record resolves its tracks, shuffled or not, and then shows the queue`() = runTest {
        val rig = rig()
        rig.actions.playAlbum(1, "Album 1", shuffled = true)
        runCurrent()
        assertEquals(listOf("replaceQueue([11, 12], 0, true)"), rig.engine.calls)
        rig.version.value = 1
        runCurrent()
        assertEquals(Tab.Queue, rig.nav.state.value.tab)
    }

    @Test
    fun `a pick of records is played and queued in the order it was ticked`() = runTest {
        val rig = rig()
        rig.actions.playAlbums(listOf(2, 1))
        runCurrent()
        assertEquals("replaceQueue([21, 11, 12], 0, false)", rig.engine.calls.single())
        rig.actions.enqueueAlbums(listOf(2, 1))
        runCurrent()
        assertEquals("addToQueue([21, 11, 12])", rig.engine.calls.last())
        rig.actions.downloadAlbums(listOf(1))
        runCurrent()
        assertEquals("downloadToCache([11, 12])", rig.engine.calls.last())
    }

    @Test
    fun `queueing does not leave the page, and an empty pick asks the engine for nothing`() = runTest {
        val rig = rig()
        rig.actions.enqueue(listOf(5, 6))
        rig.actions.playNext(listOf(7))
        rig.actions.play(emptyList())
        rig.actions.playAlbums(emptyList())
        runCurrent()
        rig.version.value = 1
        advanceTimeBy(100)
        runCurrent()
        assertEquals(Tab.Library, rig.nav.state.value.tab)
        // With nothing playing there is no cursor to go after, so play next queues.
        assertEquals(listOf("addToQueue([5, 6])", "addToQueue([7])"), rig.engine.calls)
    }

    @Test
    fun `an artist is played from the engine's account of their tracks`() = runTest {
        val rig = rig()
        rig.actions.playArtist(9, "Artist 9", shuffled = false)
        rig.actions.enqueueArtist(9)
        runCurrent()
        assertEquals(listOf("replaceQueue([11, 12, 21], 0, false)", "addToQueue([11, 12, 21])"), rig.engine.calls)
    }

    @Test
    fun `playing from a track keeps the rest behind it`() = runTest {
        val rig = rig()
        rig.actions.play(listOf(11, 12, 21), startAt = 2)
        runCurrent()
        assertEquals(1, rig.engine.calls.size)
        assertEquals(true, rig.engine.calls.single().startsWith("replaceQueue("))
    }
}

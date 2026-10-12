package cc.blit.koan.nav

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class NavigatorTest {
    private class Loader : PageLoader {
        val prepared = mutableListOf<Route>()
        val arrived = mutableListOf<Route>()
        var gate: ((Route) -> CompletableDeferred<Unit>?)? = null

        override suspend fun prepare(route: Route): Any? {
            prepared += route
            gate?.invoke(route)?.await()
            return "rows of $route"
        }

        override fun arrive(route: Route, prepared: Any?) {
            arrived += route
            assertEquals("rows of $route", prepared)
        }
    }

    private fun TestScope.navigator(loader: Loader = Loader()) = Navigator(backgroundScope, loader) to loader

    @Test
    fun `starts on the queue tab with each tab at its own root`() = runTest {
        val (nav, _) = navigator()
        val state = nav.state.value
        assertEquals(Tab.Queue, state.tab)
        assertEquals(Route.Queue, state.current)
        assertEquals(Tab.entries.map { listOf(it.root) }, Tab.entries.map { state.stacks.getValue(it) })
        assertFalse(state.canPop)
    }

    @Test
    fun `a page is read before it is shown, and the stack changes in the same step`() = runTest {
        val release = CompletableDeferred<Unit>()
        val loader = Loader().apply { gate = { release } }
        val (nav, _) = navigator(loader)
        nav.openAlbum(7)
        runCurrent()
        // Loading: the page we are leaving is still the one in the stack.
        assertEquals(Route.Queue, nav.current)
        assertEquals(Route.Album(7), nav.destination.value)
        release.complete(Unit)
        runCurrent()
        assertEquals(Route.Album(7), nav.current)
        assertNull(nav.destination.value)
        assertEquals(listOf<Route>(Route.Album(7)), loader.arrived)
    }

    @Test
    fun `the second tap while a page is loading wins`() = runTest {
        val gates = mapOf<Route, CompletableDeferred<Unit>>(Route.Album(1) to CompletableDeferred(), Route.Artist(2) to CompletableDeferred())
        val loader = Loader().apply { gate = { gates[it] } }
        val (nav, _) = navigator(loader)
        nav.openAlbum(1)
        runCurrent()
        nav.openArtist(2)
        runCurrent()
        gates.values.forEach { it.complete(Unit) }
        runCurrent()
        assertEquals(listOf<Route>(Route.Queue, Route.Artist(2)), nav.state.value.stack)
        assertEquals(listOf<Route>(Route.Artist(2)), loader.arrived)
    }

    @Test
    fun `each tab keeps its own stack while another is in front`() = runTest {
        val (nav, _) = navigator()
        nav.openAlbum(1); runCurrent()
        nav.openArtist(2); runCurrent()
        nav.selectTab(Tab.Library); runCurrent()
        nav.openPlaylist(9); runCurrent()
        assertEquals(listOf(Route.Library, Route.playlist(9)), nav.state.value.stack)
        nav.selectTab(Tab.Queue); runCurrent()
        assertEquals(listOf<Route>(Route.Queue, Route.Album(1), Route.Artist(2)), nav.state.value.stack)
        nav.selectTab(Tab.Library); runCurrent()
        assertEquals(listOf(Route.Library, Route.playlist(9)), nav.state.value.stack)
    }

    @Test
    fun `going to the page already on top pushes nothing and cancels the move in flight`() = runTest {
        val (nav, loader) = navigator()
        nav.openAlbum(1); runCurrent()
        val gate = CompletableDeferred<Unit>().also { g -> loader.gate = { if (it == Route.Artist(5)) g else null } }
        nav.openArtist(5); runCurrent()
        assertEquals(Route.Artist(5), nav.destination.value)
        nav.openAlbum(1); runCurrent() // back to the page still showing
        gate.complete(Unit); runCurrent()
        assertNull(nav.destination.value)
        assertEquals(listOf<Route>(Route.Queue, Route.Album(1)), nav.state.value.stack)
    }

    @Test
    fun `back pops the tab in front and then goes to the queue tab and then leaves the app`() = runTest {
        val (nav, _) = navigator()
        nav.selectTab(Tab.Library); runCurrent()
        nav.show(Section.Albums); runCurrent()
        nav.openAlbum(3); runCurrent()
        assertEquals(3, nav.state.value.stack.size)
        assertTrue(nav.back()); runCurrent()
        assertEquals(Route.Page(Section.Albums), nav.current)
        assertTrue(nav.back()); runCurrent()
        assertEquals(Route.Library, nav.current)
        // At a non-start tab's root, Back goes to the start tab.
        assertTrue(nav.back()); runCurrent()
        assertEquals(Tab.Queue, nav.state.value.tab)
        // And at the start tab's root the system takes over.
        assertFalse(nav.back())
    }

    @Test
    fun `choosing the tab in front again goes to its root and then to its top`() = runTest {
        val (nav, _) = navigator()
        nav.selectTab(Tab.Library); runCurrent()
        nav.openAlbum(3); runCurrent()
        nav.selectTab(Tab.Library); runCurrent()
        assertEquals(listOf<Route>(Route.Library), nav.state.value.stack)
        assertNull(nav.rewinds.value[Tab.Library])
        nav.selectTab(Tab.Library); runCurrent()
        assertEquals(1, nav.rewinds.value[Tab.Library])
        nav.selectTab(Tab.Library); runCurrent()
        assertEquals(2, nav.rewinds.value[Tab.Library])
    }

    @Test
    fun `a tab's own page brings that tab forward at its root`() = runTest {
        val (nav, _) = navigator()
        nav.openAlbum(1); runCurrent()
        nav.selectTab(Tab.Search); runCurrent()
        nav.openArtist(4); runCurrent()
        nav.go(Route.Queue); runCurrent()
        assertEquals(Tab.Queue, nav.state.value.tab)
        assertEquals(listOf<Route>(Route.Queue), nav.state.value.stack)
        // The search tab's stack is not the queue's business.
        assertEquals(listOf<Route>(Route.Search, Route.Artist(4)), nav.state.value.stacks.getValue(Tab.Search))
    }

    @Test
    fun `forgetting a deleted playlist takes it out of every stack but never a root`() = runTest {
        val (nav, loader) = navigator()
        nav.openPlaylist(9); runCurrent()
        nav.selectTab(Tab.Library); runCurrent()
        nav.openPlaylist(9); runCurrent()
        nav.openAlbum(2); runCurrent()
        nav.forget(Section.Playlist(9)); runCurrent()
        assertEquals(listOf<Route>(Route.Queue), nav.state.value.stacks.getValue(Tab.Queue))
        assertEquals(listOf(Route.Library, Route.Album(2)), nav.state.value.stacks.getValue(Tab.Library))
        // The models are brought to the page now showing.
        assertEquals(Route.Album(2), loader.arrived.last())
        nav.forget(Section.Queue)
        assertEquals(Route.Queue, nav.state.value.stacks.getValue(Tab.Queue).single())
    }

    @Test
    fun `a record opened for a track remembers which until the page clears it`() = runTest {
        val (nav, _) = navigator()
        nav.openAlbum(1, highlighting = 42); runCurrent()
        assertEquals(42L, nav.highlightedTrackId.value)
        nav.clearHighlight()
        assertNull(nav.highlightedTrackId.value)
    }

    @Test
    fun `a failing read still lets the move land`() = runTest {
        val loader = object : PageLoader {
            override suspend fun prepare(route: Route): Any? = throw IllegalStateException("boom")
            override fun arrive(route: Route, prepared: Any?) = Unit
        }
        val nav = Navigator(backgroundScope, loader)
        nav.openAlbum(1); runCurrent()
        assertEquals(Route.Album(1), nav.current)
    }

    @Test
    fun `the queue is shown once it holds what was just started, and not if that is slow`() = runTest {
        val (nav, _) = navigator()
        nav.selectTab(Tab.Library); runCurrent()
        val version = MutableStateFlow(1L)
        nav.showQueueWhenReady(version, waitMs = 50)
        runCurrent()
        version.value = 2
        runCurrent()
        assertEquals(Tab.Queue, nav.state.value.tab)

        nav.selectTab(Tab.Library); runCurrent()
        nav.showQueueWhenReady(version, waitMs = 50)
        advanceTimeBy(60)
        version.value = 3
        runCurrent()
        assertEquals(Tab.Library, nav.state.value.tab)
    }
}

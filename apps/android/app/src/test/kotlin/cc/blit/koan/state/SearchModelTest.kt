package cc.blit.koan.state

import cc.blit.koan.FakeEngine
import cc.blit.koan.album
import cc.blit.koan.artist
import cc.blit.koan.nav.Navigator
import cc.blit.koan.record
import cc.blit.koan.track
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.KoanException
import uniffi.koan_ffi.ShelfKind
import uniffi.koan_ffi.ShelfSummary
import java.lang.reflect.Proxy
import kotlin.coroutines.Continuation
import kotlin.coroutines.intrinsics.COROUTINE_SUSPENDED
import kotlin.coroutines.resume

@OptIn(ExperimentalCoroutinesApi::class)
class SearchModelTest {
    private val mirror = EngineMirror()

    private fun summary(vararg ids: Long, total: ULong = 40uL) = ShelfSummary(
        artists = listOf(artist(1)), artistTotal = 1uL,
        albums = listOf(album(2)), albumTotal = 7uL,
        tracks = ids.map { track(it) }, trackTotal = total,
    )

    private fun TestScope.model(engine: KoanEngineInterface) = SearchModel(
        engine, mirror, PlayerModel(engine, mirror, backgroundScope), Navigator(backgroundScope), backgroundScope, debounceMs = 160,
    )

    /** Engine whose shelf searches wait until the test answers them, so a slow answer can land late. */
    private class Gated {
        val asked = mutableListOf<String>()
        private val waiting = mutableListOf<Continuation<Any?>>()
        val engine: KoanEngineInterface = Proxy.newProxyInstance(
            KoanEngineInterface::class.java.classLoader, arrayOf(KoanEngineInterface::class.java),
        ) { _, method, args ->
            @Suppress("UNCHECKED_CAST")
            when (method.name) {
                "shelfSummary" -> {
                    asked += ((args!![0]) as ShelfKind.Search).query
                    waiting += args.last() as Continuation<Any?>
                    COROUTINE_SUSPENDED
                }
                "toString" -> "Gated"
                "hashCode" -> 0
                "equals" -> false
                else -> null
            }
        } as KoanEngineInterface

        fun answer(index: Int, value: Any?) = waiting[index].resume(value)
    }

    private fun FakeEngine.searches() = synchronized(calls) { calls.filter { it.startsWith("shelfSummary") } }

    @Test
    fun `typing is debounced into one search for the last text`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "shelfSummary") summary(10) else null }
        val search = model(engine.engine)
        search.setQuery("m")
        advanceTimeBy(50)
        search.setQuery("mi")
        advanceTimeBy(50)
        search.setQuery("mil")
        runCurrent()
        assertTrue(engine.searches().isEmpty())
        assertTrue("it is searching while it waits", search.results.value.searching)
        advanceTimeBy(161)
        runCurrent()

        assertEquals(1, engine.searches().size)
        assertTrue(engine.searches().single().contains("mil"))
        val results = search.results.value
        assertFalse(results.searching)
        assertEquals("mil", results.query)
        assertEquals(listOf(10L), results.tracks.map { it.id })
        // The heading numbers are the listings', not the previews'.
        assertEquals(ShelfTotals(1, 7, 40), results.totals)
    }

    @Test
    fun `a slow answer to an old query never lands over the newer one`() = runTest {
        val gate = Gated()
        val search = model(gate.engine)
        search.setQuery("mil")
        advanceTimeBy(161)
        runCurrent()
        search.setQuery("miles")
        advanceTimeBy(161)
        runCurrent()
        assertEquals(listOf("mil", "miles"), gate.asked)

        gate.answer(1, summary(2))
        runCurrent()
        assertEquals(listOf(2L), search.results.value.tracks.map { it.id })
        // The first, late: it belongs to a generation that is gone.
        gate.answer(0, summary(1))
        runCurrent()
        assertEquals(listOf(2L), search.results.value.tracks.map { it.id })
        assertEquals("miles", search.results.value.query)
    }

    @Test
    fun `clearing cancels what is under way and drops a late answer`() = runTest {
        val gate = Gated()
        val search = model(gate.engine)
        search.setQuery("mil")
        advanceTimeBy(161)
        runCurrent()
        search.clear()
        assertFalse(search.hasQuery.value)
        assertEquals("", search.query.value)

        gate.answer(0, summary(1))
        runCurrent()
        assertEquals(SearchResults(), search.results.value)
    }

    @Test
    fun `blank text is no query and asks nothing`() = runTest {
        val engine = FakeEngine { _, _ -> summary(1) }
        val search = model(engine.engine)
        search.setQuery("   ")
        advanceTimeBy(500)
        runCurrent()
        assertFalse(search.hasQuery.value)
        assertTrue(engine.searches().isEmpty())
        assertFalse(search.results.value.searching)
    }

    @Test
    fun `the query is sent trimmed but the field keeps what was typed`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "shelfSummary") summary() else null }
        val search = model(engine.engine)
        search.setQuery(" miles ")
        advanceTimeBy(161)
        runCurrent()
        assertEquals(" miles ", search.query.value)
        assertEquals("miles", search.results.value.query)
    }

    @Test
    fun `a failed search finds nothing rather than failing the page`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "shelfSummary") throw KoanException.Audio("boom") else null }
        val search = model(engine.engine)
        search.setQuery("x")
        advanceTimeBy(161)
        runCurrent()
        assertTrue(search.results.value.isEmpty)
        assertFalse(search.results.value.searching)
        assertNull(search.results.value.totals)
    }

    @Test
    fun `hasQuery is written only when it flips`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "shelfSummary") summary() else null }
        val search = model(engine.engine)
        val seen = search.hasQuery.record(backgroundScope)
        search.setQuery("a")
        search.setQuery("ab")
        search.setQuery("abc")
        search.setQuery("")
        runCurrent()
        assertEquals(listOf(false, true, false), seen.toList())
    }

    @Test
    fun `a pick across kinds plays what each resolves to`() = runTest {
        val engine = FakeEngine { name, args ->
            when (name) {
                "shelfSummary" -> summary(10)
                "trackIds" -> if (args[0] != null) listOf(21L, 22L) else listOf(31L)
                else -> null
            }
        }
        val search = model(engine.engine)
        search.setQuery("x")
        advanceTimeBy(161)
        runCurrent()
        val results = search.results.value
        search.selection.begin(Playable.OfAlbum(results.albums.single()))
        search.selection.toggle(Playable.OfArtist(1, "Artist 1"))
        search.selection.toggle(Playable.OfTrack(results.tracks.single()))
        search.commit(search.selection, play = true)
        runCurrent()

        assertFalse("the page is back to normal", search.selection.isActive)
        // In the order ticked: the record's tracks, the artist's, the track.
        val replace = synchronized(engine.calls) { engine.calls.single { it.startsWith("replaceQueue") } }
        assertTrue(replace, replace.contains("[21, 22, 31, 10]"))
    }

    @Test
    fun `queueing a pick appends instead`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "trackIds") listOf(5L) else null }
        val search = model(engine.engine)
        search.selection.begin(Playable.OfAlbum(album(2)))
        search.commit(search.selection, play = false)
        runCurrent()
        val calls = synchronized(engine.calls) { engine.calls.toList() }
        assertTrue(calls.any { it.startsWith("addToQueue") && it.contains("[5]") })
        assertTrue(calls.none { it.startsWith("replaceQueue") })
    }
}

package cc.blit.koan.state

import cc.blit.koan.FakeEngine
import cc.blit.koan.playlist
import cc.blit.koan.ui.playlists.PlaylistLayout
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * What the playlist pages ask of `PlaylistsModel`, checked against the model
 * and a recording engine: that a drag becomes the call that lands the row
 * where it was dropped, that a read-only playlist is refused whatever the page
 * sends, and the naming request.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class PlaylistEditsTest {
    private val mirror = EngineMirror()

    private fun TestScope.model(engine: FakeEngine) = PlaylistsModel(engine.engine, mirror, backgroundScope)

    private fun FakeEngine.callsTo(prefix: String) = synchronized(calls) { calls.filter { it.startsWith(prefix) } }

    private fun lists(vararg ps: uniffi.koan_ffi.Playlist) = FakeEngine { name, _ ->
        when (name) {
            "playlists" -> ps.toList()
            "playlistCoverAlbumIds" -> emptyList<Long>()
            "createPlaylist" -> playlist(99)
            else -> null
        }
    }

    @Test
    fun `dragging a playlist down sends the rows it passed to land before it, and the model shows the order at once`() = runTest {
        val engine = lists(playlist(1), playlist(2), playlist(3), playlist(4))
        val model = model(engine)
        model.load(); runCurrent()
        val order = model.playlists.value.map { it.id }
        val move = PlaylistLayout.movePlaylist(order, from = 0, to = 3)!!
        model.reorderPlaylists(move.moving, move.onto); runCurrent()
        assertEquals(listOf(2L, 3L, 4L, 1L), model.playlists.value.map { it.id })
        assertEquals(listOf("reorderPlaylists([2, 3, 4, 1])"), engine.callsTo("reorderPlaylists"))
    }

    @Test
    fun `dragging a playlist up lands it before the one it was dropped on`() = runTest {
        val engine = lists(playlist(1), playlist(2), playlist(3))
        val model = model(engine)
        model.load(); runCurrent()
        val move = PlaylistLayout.movePlaylist(model.playlists.value.map { it.id }, from = 2, to = 0)!!
        model.reorderPlaylists(move.moving, move.onto); runCurrent()
        assertEquals(listOf(3L, 1L, 2L), model.playlists.value.map { it.id })
    }

    @Test
    fun `a read-only playlist refuses every edit the page could send, and a plain one takes them`() = runTest {
        val engine = lists(playlist(1, readonly = true), playlist(2))
        val model = model(engine)
        model.load(); runCurrent()
        assertEquals(false, model.fillable(1))
        model.reorder(listOf(2, 1), inPlaylist = 1)
        model.remove(listOf(1), from = 1)
        model.insert(listOf(5), into = 1, at = 0)
        model.add(listOf(5), to = 1)
        model.shuffle(1)
        runCurrent()
        assertEquals(emptyList<String>(), engine.callsTo("reorderPlaylist(").plus(engine.callsTo("removeFromPlaylist"))
            .plus(engine.callsTo("insertIntoPlaylist")).plus(engine.callsTo("addToPlaylist")).plus(engine.callsTo("shufflePlaylist")))
        model.reorder(listOf(2, 1), inPlaylist = 2)
        model.remove(listOf(1), from = 2)
        model.setGrouped(true, id = 1) // a layout is a view, not an edit: it is remembered even on a read-only playlist
        runCurrent()
        assertEquals(1, engine.callsTo("reorderPlaylist(").size)
        assertEquals(1, engine.callsTo("removeFromPlaylist").size)
        assertEquals(1, engine.callsTo("setPlaylistGrouped").size)
        assertEquals(true, model.playlists.value.first { it.id == 1L }.grouped)
    }

    @Test
    fun `a playlist read from a file is not renamed`() = runTest {
        val engine = lists(playlist(1, fromFile = true), playlist(2))
        val model = model(engine)
        model.load(); runCurrent()
        model.rename(1, "New name")
        model.rename(2, "  New name ")
        runCurrent()
        assertEquals(listOf("renamePlaylist(2, New name)"), engine.callsTo("renamePlaylist"))
    }

    @Test
    fun `naming is a request, empty means a new empty playlist, and answering it creates, trimmed`() = runTest {
        val engine = lists(playlist(1))
        val model = model(engine)
        assertNull(model.naming.value)
        model.beginNaming()
        assertEquals(emptyList<Long>(), model.naming.value)
        model.beginNaming(listOf(4, 5))
        assertEquals(listOf(4L, 5L), model.naming.value)
        val request = model.naming.value!!
        model.endNaming()
        assertNull(model.naming.value)
        assertNull("a blank name makes nothing", model.create("   ", request))
        val made = model.create("  Road trip ", request)
        assertNotNull(made)
        assertEquals(listOf("createPlaylist(Road trip, [4, 5])"), engine.callsTo("createPlaylist"))
    }

    @Test
    fun `a failed create is reported for the page to show, once`() = runTest {
        val engine = FakeEngine { name, _ -> if (name == "createPlaylist") throw uniffi.koan_ffi.KoanException.Database("disk full") else null }
        val model = model(engine)
        assertNull(model.create("Mix"))
        assertNotNull(model.report.value)
        model.dismissReport()
        assertNull(model.report.value)
    }
}

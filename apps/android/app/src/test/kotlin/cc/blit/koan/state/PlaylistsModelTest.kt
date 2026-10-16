package cc.blit.koan.state

import cc.blit.koan.FakeEngine
import cc.blit.koan.playlist
import cc.blit.koan.art.ArtworkSource
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.KoanException
import uniffi.koan_ffi.PlaylistEntry
import uniffi.koan_ffi.StateSlice

@OptIn(ExperimentalCoroutinesApi::class)
class PlaylistsModelTest {
    private val mirror = EngineMirror()

    private fun TestScope.model(engine: FakeEngine) = PlaylistsModel(engine.engine, mirror, backgroundScope)

    private fun FakeEngine.count(prefix: String) = synchronized(calls) { calls.count { it.startsWith(prefix) } }

    private fun lists(vararg ps: uniffi.koan_ffi.Playlist) = FakeEngine { name, _ ->
        when (name) {
            "playlists" -> ps.toList()
            "playlistCoverAlbumIds" -> listOf(1L, 2L)
            else -> null
        }
    }

    @Test
    fun `the list is read whole, with a mosaic of up to four records for each`() = runTest {
        val engine = lists(playlist(1), playlist(2))
        val model = model(engine)
        model.load(); runCurrent()
        assertEquals(listOf(1L, 2L), model.playlists.value.map { it.id })
        assertEquals(listOf<ArtworkSource>(ArtworkSource.Album(1), ArtworkSource.Album(2)), model.covers.value.getValue(1))
        assertEquals(2, engine.count("playlistCoverAlbumIds("))
    }

    @Test
    fun `a mosaic is read again only for a playlist whose contents changed`() = runTest {
        var second = playlist(2, changedAt = "t0")
        val engine = FakeEngine { name, _ ->
            when (name) {
                "playlists" -> listOf(playlist(1), second)
                "playlistCoverAlbumIds" -> listOf(1L)
                else -> null
            }
        }
        val model = model(engine)
        model.load(); runCurrent()
        model.load(); runCurrent()
        assertEquals("nothing changed, nothing read", 2, engine.count("playlistCoverAlbumIds("))
        second = playlist(2, changedAt = "t1")
        model.load(); runCurrent()
        assertEquals(3, engine.count("playlistCoverAlbumIds("))
    }

    @Test
    fun `a deleted playlist stops holding its mosaic`() = runTest {
        var present = listOf(playlist(1), playlist(2))
        val engine = FakeEngine { name, _ ->
            when (name) {
                "playlists" -> present
                "playlistCoverAlbumIds" -> listOf(1L)
                else -> null
            }
        }
        val model = model(engine)
        model.load(); runCurrent()
        present = listOf(playlist(1))
        model.load(); runCurrent()
        assertEquals(setOf(1L), model.covers.value.keys)
    }

    @Test
    fun `a playlist's rows and mosaic land together, and again only when the library moved`() = runTest {
        val engine = FakeEngine { name, _ ->
            when (name) {
                "playlistTracks" -> listOf(PlaylistEntry(10, cc.blit.koan.track(1)), PlaylistEntry(11, cc.blit.koan.track(1)))
                "playlistCoverAlbumIds" -> listOf(5L)
                else -> null
            }
        }
        val model = model(engine)
        model.prepare(3)
        assertEquals(3L, model.openId.value)
        // The same track twice: entries, not tracks, name the rows.
        assertEquals(listOf(10L, 11L), model.entries.value.map { it.entryId })
        assertEquals(listOf<ArtworkSource>(ArtworkSource.Album(5)), model.covers.value[3])
        model.prepare(3)
        assertEquals(1, engine.count("playlistTracks("))
        mirror.apply(StateSlice.Library(1uL))
        model.prepare(3)
        assertEquals(2, engine.count("playlistTracks("))
        assertTrue(!model.isLoading.value)
    }

    @Test
    fun `deleting the open playlist empties the page it was on`() = runTest {
        val engine = lists(playlist(3))
        val model = model(engine)
        model.load(); runCurrent()
        model.prepare(3)
        model.delete(3); runCurrent()
        assertNull(model.openId.value)
        assertEquals(emptyList<PlaylistEntry>(), model.entries.value)
        assertEquals(1, engine.count("deletePlaylist("))
    }

    @Test
    fun `a read-only playlist refuses every edit to what it holds`() = runTest {
        val engine = lists(playlist(1, readonly = true), playlist(2))
        val model = model(engine)
        model.load(); runCurrent()
        model.add(listOf(9), to = 1)
        model.insert(listOf(9), into = 1, at = 0)
        model.reorder(listOf(1, 2), inPlaylist = 1)
        model.remove(listOf(1), from = 1)
        model.shuffle(1)
        runCurrent()
        assertEquals(0, engine.count("addToPlaylist(") + engine.count("insertIntoPlaylist(") + engine.count("reorderPlaylist(") +
            engine.count("removeFromPlaylist(") + engine.count("shufflePlaylist("))
        model.add(listOf(9), to = 2)
        model.add(emptyList(), to = 2)
        runCurrent()
        assertEquals(listOf("addToPlaylist(2, [9])"), engine.calls.filter { it.startsWith("addToPlaylist(") })
    }

    @Test
    fun `a playlist from a file is named by its file`() = runTest {
        val engine = lists(playlist(1, fromFile = true), playlist(2))
        val model = model(engine)
        model.load(); runCurrent()
        model.rename(1, "new"); model.rename(2, "  "); model.rename(2, " New name "); runCurrent()
        assertEquals(listOf("renamePlaylist(2, New name)"), engine.calls.filter { it.startsWith("renamePlaylist(") })
    }

    @Test
    fun `reordering the list shows at once and then tells the engine, and onto itself is nothing`() = runTest {
        val engine = lists(playlist(1), playlist(2), playlist(3), playlist(4))
        val model = model(engine)
        model.load(); runCurrent()
        model.reorderPlaylists(listOf(4, 3), onto = 1); runCurrent()
        assertEquals(listOf(4L, 3L, 1L, 2L), model.playlists.value.map { it.id })
        assertEquals("reorderPlaylists(4, 3, 1, 2)".replace("4, 3, 1, 2", "[4, 3, 1, 2]"), engine.calls.last { it.startsWith("reorderPlaylists(") })
        val before = engine.count("reorderPlaylists(")
        model.reorderPlaylists(listOf(3), onto = 3); runCurrent()
        assertEquals(before, engine.count("reorderPlaylists("))
    }

    @Test
    fun `creating a playlist trims the name, refuses an empty one and reports a failure`() = runTest {
        val engine = FakeEngine { name, _ ->
            if (name == "createPlaylist") playlist(7) else null
        }
        val model = model(engine)
        assertNull(model.create("   "))
        assertNotNull(model.create("  Road trip ", listOf(1, 2)))
        assertEquals(listOf("createPlaylist(Road trip, [1, 2])"), engine.calls)

        val failing = FakeEngine { name, _ -> if (name == "createPlaylist") throw KoanException.Audio("disk full") else null }
        val other = model(failing)
        assertNull(other.create("x"))
        assertTrue(other.report.value!!.startsWith("Couldn't create that playlist"))
        other.dismissReport()
        assertNull(other.report.value)
    }

    @Test
    fun `an edit that fails is reported, not thrown`() = runTest {
        val engine = FakeEngine { name, _ ->
            when (name) {
                "playlists" -> listOf(playlist(1))
                "playlistCoverAlbumIds" -> emptyList<Long>()
                "addToPlaylist" -> throw KoanException.Audio("nope")
                else -> null
            }
        }
        val model = model(engine)
        model.load(); runCurrent()
        model.add(listOf(1), to = 1); runCurrent()
        assertNotNull(model.report.value)
    }

    @Test
    fun `naming is a request that outlives the menu that made it`() = runTest {
        val model = model(FakeEngine())
        assertNull(model.naming.value)
        model.beginNaming(listOf(1, 2))
        assertEquals(listOf(1L, 2L), model.naming.value)
        model.endNaming()
        assertNull(model.naming.value)
        model.beginNaming()
        assertEquals(emptyList<Long>(), model.naming.value)
    }
}

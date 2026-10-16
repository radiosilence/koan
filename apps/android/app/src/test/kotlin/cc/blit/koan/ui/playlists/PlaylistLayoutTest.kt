package cc.blit.koan.ui.playlists

import cc.blit.koan.playlist
import cc.blit.koan.track
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.PlaylistEntry

class PlaylistLayoutTest {
    private fun entry(entryId: Long, trackId: Long = entryId, album: Long? = 1, title: String = "Album ${album ?: 0}") =
        PlaylistEntry(entryId, track(trackId, album).copy(albumTitle = title))

    private fun shape(lines: List<PlaylistLine>) = lines.map {
        when (it) {
            is PlaylistLine.Heading -> "H${it.positions}"
            is PlaylistLine.Entry -> "E${it.entry.entryId}"
        }
    }

    @Test
    fun `flat is one row per entry, and a track held twice is two rows with two identities`() {
        val entries = listOf(entry(10, trackId = 5), entry(11, trackId = 5), entry(12, trackId = 6))
        val lines = PlaylistLayout.build(entries, grouped = false)
        assertEquals(listOf("E10", "E11", "E12"), shape(lines))
        assertEquals(3, lines.map { it.id }.toSet().size)
        assertTrue(lines.all { it.contentType == "entry" })
    }

    @Test
    fun `grouped puts a heading over each contiguous run, so a second visit to a record is a second heading`() {
        val entries = listOf(
            entry(1, album = 1), entry(2, album = 1), entry(3, album = 2), entry(4, album = 1),
        )
        val lines = PlaylistLayout.build(entries, grouped = true)
        assertEquals(listOf("H[0, 1]", "E1", "E2", "H[2]", "E3", "H[3]", "E4"), shape(lines))
        assertEquals("heading", lines[0].contentType)
        assertEquals("album:1", lines[0].id)
    }

    @Test
    fun `an entry with no album takes no heading, and records with no id are told apart by title and artist`() {
        val entries = listOf(
            entry(1, album = null, title = ""),
            entry(2, album = null, title = "Live"), entry(3, album = null, title = "Live"), entry(4, album = null, title = "Studio"),
        )
        assertEquals(listOf("E1", "H[1, 2]", "E2", "E3", "H[3]", "E4"), shape(PlaylistLayout.build(entries, grouped = true)))
    }

    @Test
    fun `a heading stands for its whole run when picked, and positions come back in playlist order`() {
        val entries = listOf(entry(1, album = 1), entry(2, album = 1), entry(3, album = 2))
        val lines = PlaylistLayout.build(entries, grouped = true)
        assertEquals(listOf(0, 1), PlaylistLayout.positions(lines, setOf("album:1")))
        assertEquals(listOf(0, 1, 2), PlaylistLayout.positions(lines, setOf(entryKey(3), "album:1", entryKey(2))))
        assertEquals(emptyList<Int>(), PlaylistLayout.positions(lines, emptySet()))
        assertEquals(listOf(2L, 3L), PlaylistLayout.entryIds(entries, listOf(1, 2)))
        assertEquals(listOf(2L), PlaylistLayout.trackIds(entries, listOf(1, 9)))
    }

    @Test
    fun `a single dragged entry is the order its row was dropped in`() {
        val entries = (1L..4L).map { entry(it) }
        val shown = PlaylistLayout.build(entries, false).toMutableList().apply { add(0, removeAt(2)) }
        assertEquals(listOf(3L, 1L, 2L, 4L), PlaylistLayout.reordered(entries, shown, dragged = 3, picked = emptySet()))
    }

    @Test
    fun `a drop that leaves everything where it was sends nothing`() {
        val entries = (1L..3L).map { entry(it) }
        assertNull(PlaylistLayout.reordered(entries, PlaylistLayout.build(entries, false), dragged = 2, picked = emptySet()))
    }

    @Test
    fun `dragging one of several picked entries carries the others with it, in their old order`() {
        val entries = (1L..6L).map { entry(it) }
        // 5 dragged to the front while 2 and 5 are picked.
        val shown = PlaylistLayout.build(entries, false).toMutableList().apply { add(0, removeAt(4)) }
        assertEquals(
            listOf(2L, 5L, 1L, 3L, 4L, 6L),
            PlaylistLayout.reordered(entries, shown, dragged = 5, picked = setOf(2, 5)),
        )
        // And dragged to the end.
        val toEnd = PlaylistLayout.build(entries, false).toMutableList().apply { add(removeAt(0)) }
        assertEquals(
            listOf(3L, 4L, 5L, 6L, 1L, 2L),
            PlaylistLayout.reordered(entries, toEnd, dragged = 1, picked = setOf(1, 2)),
        )
    }

    @Test
    fun `in a grouped list headings drop out of the order sent`() {
        val entries = listOf(entry(1, album = 1), entry(2, album = 1), entry(3, album = 2))
        val shown = PlaylistLayout.build(entries, true).toMutableList()
        // [H1, E1, E2, H2, E3]: E3 dragged above H2 and E2 -> [H1, E1, E3, E2, H2]? Only entry order counts.
        val e3 = shown.removeAt(4)
        shown.add(2, e3)
        assertEquals(listOf(1L, 3L, 2L), PlaylistLayout.reordered(entries, shown, dragged = 3, picked = emptySet()))
    }

    @Test
    fun `a shown list that has lost or gained entries is not sent`() {
        val entries = (1L..3L).map { entry(it) }
        val shown = PlaylistLayout.build(entries.take(2), false)
        assertNull(PlaylistLayout.reordered(entries, shown, dragged = 1, picked = emptySet()))
    }

    /** What `PlaylistsModel.reorderPlaylists` does with its arguments, to check the mapping end to end. */
    private fun applyModel(order: List<Long>, move: PlaylistMove): List<Long> {
        val rest = order.filterNot { it in move.moving }.toMutableList()
        rest.addAll(rest.indexOf(move.onto), move.moving)
        return rest
    }

    @Test
    fun `every move of a playlist, up or down, lands where it was dropped`() {
        val order = listOf(10L, 20L, 30L, 40L, 50L)
        for (from in order.indices) for (to in order.indices) {
            val move = PlaylistLayout.movePlaylist(order, from, to)
            if (from == to) {
                assertNull(move)
                continue
            }
            val expected = order.toMutableList().apply { add(to, removeAt(from)) }
            assertEquals("$from -> $to", expected, applyModel(order, move!!))
        }
    }

    @Test
    fun `a move off the list is nothing`() {
        assertNull(PlaylistLayout.movePlaylist(listOf(1L, 2L), 0, 2))
        assertNull(PlaylistLayout.movePlaylist(listOf(1L, 2L), -1, 1))
    }

    @Test
    fun `a read-only playlist takes no edits, and one read from a file cannot be renamed`() {
        assertTrue(PlaylistLayout.fillable(playlist(1)))
        assertFalse(PlaylistLayout.fillable(playlist(2, readonly = true)))
        assertTrue(PlaylistLayout.renamable(playlist(1)))
        assertTrue("a smart playlist keeps its name editable, as the model has it", PlaylistLayout.renamable(playlist(2, readonly = true)))
        assertFalse(PlaylistLayout.renamable(playlist(3, fromFile = true)))
        assertFalse(PlaylistLayout.renamable(null))
    }

    @Test
    fun `the summary says how many, how long, and whether a server keeps it`() {
        val entries = listOf(entry(1), entry(2))
        assertEquals("2 tracks · 6:00 · synced", PlaylistLayout.summary(entries, synced = true))
        assertEquals("1 track · 3:00", PlaylistLayout.summary(entries.take(1), synced = false))
        assertEquals("0 tracks", PlaylistLayout.summary(emptyList(), synced = false))
        assertEquals("1:02:09", PlaylistLayout.duration(3_729_000))
    }

    @Test
    fun `a name is trimmed and an empty one is refused`() {
        assertEquals("Road trip", PlaylistLayout.nameable("  Road trip \n"))
        assertNull(PlaylistLayout.nameable("   "))
        assertEquals("Delete \"Mix\"? The tracks stay in the library.", PlaylistLayout.deleteQuestion(playlist(1).copy(name = "Mix")))
    }
}

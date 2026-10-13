package cc.blit.koan.ui.queue

import cc.blit.koan.item
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.QueueItem

private fun qi(
    id: String,
    album: String = "Record",
    albumArtist: String = "Band",
    title: String = "Song $id",
    artist: String = albumArtist,
): QueueItem = item(id).copy(album = album, albumArtist = albumArtist, title = title, artist = artist)

class QueueLayoutTest {
    @Test
    fun `grouped, a heading precedes each contiguous run of one record`() {
        val items = listOf(qi("a1"), qi("a2"), qi("b1", album = "Other"), qi("b2", album = "Other"), qi("a3"), qi("a4"))
        val lines = QueueLayout.build(items, grouped = true)
        assertEquals(
            listOf("album:a1", "a1", "a2", "album:b1", "b1", "b2", "album:a3", "a3", "a4"),
            lines.map { it.id },
        )
        // The same record visited twice is two headings: queue order is the user's.
        assertEquals(2, lines.count { it is QueueLine.Heading && it.group.album == "Record" })
    }

    @Test
    fun `a record is its albumArtist too, so a same-named album by someone else is a new run`() {
        val lines = QueueLayout.build(listOf(qi("a1"), qi("a2"), qi("c1", albumArtist = "Cover Band")), true)
        assertEquals(listOf("album:a1", "a1", "a2", "album:c1", "c1"), lines.map { it.id })
    }

    @Test
    fun `a track with no album stands alone and shows its artist`() {
        val lines = QueueLayout.build(listOf(qi("x", album = "", artist = "Someone"), qi("y")), true)
        val x = lines[0] as QueueLine.Track
        assertEquals("x", x.id)
        assertTrue(x.showArtist)
        assertFalse(x.artwork)
        assertTrue(lines[1] is QueueLine.Heading)
    }

    @Test
    fun `a record that is only its own title folds into one row with a sleeve`() {
        val lines = QueueLayout.build(listOf(qi("s", album = "Single ", title = "single"), qi("t")), true)
        val s = lines[0] as QueueLine.Track
        assertTrue(s.artwork)
        assertTrue(s.showArtist)
        assertEquals("trackArt", s.contentType)
        assertEquals(listOf("s", "album:t", "t"), lines.map { it.id })
    }

    @Test
    fun `the artist shows under a track only when it differs from the album artist`() {
        val lines = QueueLayout.build(listOf(qi("a1"), qi("a2", artist = "Guest")), true)
        assertFalse((lines[1] as QueueLine.Track).showArtist)
        assertTrue((lines[2] as QueueLine.Track).showArtist)
    }

    @Test
    fun `ungrouped is one sleeve row per track`() {
        val lines = QueueLayout.build(listOf(qi("a1"), qi("a2")), grouped = false)
        assertEquals(listOf("a1", "a2"), lines.map { it.id })
        assertTrue(lines.all { it is QueueLine.Track && it.artwork && it.showArtist })
    }

    @Test
    fun `every row has a content type and headings differ from tracks`() {
        val lines = QueueLayout.build(listOf(qi("a1"), qi("a2")), true)
        assertEquals(listOf("heading", "track", "track"), lines.map { it.contentType })
    }

    @Test
    fun `an empty queue has no rows`() {
        assertEquals(emptyList<QueueLine>(), QueueLayout.build(emptyList(), true))
    }

    @Test
    fun `a heading stands for its whole run when selected`() {
        val lines = QueueLayout.build(listOf(qi("a1"), qi("a2"), qi("b1", album = "Other")), true)
        assertEquals(listOf("a1", "a2"), QueueLayout.itemIds(setOf("album:a1"), lines))
        assertEquals(listOf("b1"), QueueLayout.itemIds(setOf("b1"), lines))
    }

    @Test
    fun `indexOfItem finds the row of a queue item`() {
        val lines = QueueLayout.build(listOf(qi("a1"), qi("a2")), true)
        assertEquals(2, QueueLayout.indexOfItem(lines, "a2"))
        assertEquals(-1, QueueLayout.indexOfItem(lines, "zzz"))
        assertEquals(-1, QueueLayout.indexOfItem(lines, null))
    }

    // MARK: - Drag reorder

    private val queue = listOf("a1", "a2", "a3", "b1")
    private val lines = QueueLayout.build(queue.map { qi(it, album = if (it.startsWith("a")) "Record" else "Other") }, true)

    /** The rows as left after a drag: [id] moved to sit at [to]. */
    private fun dragged(id: String, to: Int): List<QueueLine> =
        lines.toMutableList().apply { val at = indexOfFirst { it.id == id }; add(to, removeAt(at)) }

    @Test
    fun `dropping a track lands before the first row below it that is not moving`() {
        // a3 dragged up above a2: rows are [album:a1, a1, a3, a2, album:b1, b1].
        val op = QueueLayout.moveOp(dragged("a3", 2), "a3", emptySet(), queue)!!
        assertEquals(MoveOp(listOf("a3"), "a2", after = false), op)
        assertEquals(listOf("a1", "a3", "a2", "b1"), QueueLayout.resulting(queue, op))
    }

    @Test
    fun `dropping at the very top is expressible`() {
        val op = QueueLayout.moveOp(dragged("a3", 0), "a3", emptySet(), queue)!!
        assertEquals(MoveOp(listOf("a3"), "a1", after = false), op)
        assertEquals(listOf("a3", "a1", "a2", "b1"), QueueLayout.resulting(queue, op))
    }

    @Test
    fun `dropping past the last row moves to the end, after the last that stays`() {
        val op = QueueLayout.moveOp(dragged("a1", lines.size - 1), "a1", emptySet(), queue)!!
        assertEquals(MoveOp(listOf("a1"), "b1", after = true), op)
        assertEquals(listOf("a2", "a3", "b1", "a1"), QueueLayout.resulting(queue, op))
    }

    @Test
    fun `dropping a track below the next record's heading lands it before that record's first`() {
        // a2 dragged to just under album:b1: rows are [album:a1, a1, a3, album:b1, a2, b1].
        val op = QueueLayout.moveOp(dragged("a2", 4), "a2", emptySet(), queue)!!
        assertEquals(MoveOp(listOf("a2"), "b1", after = false), op)
        assertEquals(listOf("a1", "a3", "a2", "b1"), QueueLayout.resulting(queue, op))
    }

    @Test
    fun `a row dropped where it began is no edit`() {
        assertNull(QueueLayout.moveOp(lines, "a2", emptySet(), queue))
        // Nor is a heading hop that leaves the order the same.
        assertNull(QueueLayout.moveOp(dragged("a3", lines.indexOfFirst { it.id == "b1" } - 1), "a3", emptySet(), queue))
    }

    @Test
    fun `dragging one of several selected rows moves them all, in queue order`() {
        val rows = dragged("a3", 0)
        val op = QueueLayout.moveOp(rows, "a3", setOf("a3", "a1", "b1"), queue)!!
        assertEquals(listOf("a1", "a3", "b1"), op.itemIds)
        assertEquals("a2", op.target)
        assertEquals(listOf("a1", "a3", "b1", "a2"), QueueLayout.resulting(queue, op))
    }

    @Test
    fun `dragging a row that is not selected moves only it`() {
        val op = QueueLayout.moveOp(dragged("a3", 0), "a3", setOf("a1", "b1"), queue)!!
        assertEquals(listOf("a3"), op.itemIds)
    }

    @Test
    fun `an unknown row is no edit`() {
        assertNull(QueueLayout.moveOp(lines, "nope", emptySet(), queue))
    }

    // MARK: - Selection

    @Test
    fun `toggling starts selecting and picks and unpicks`() {
        var s = QueueSelection()
        assertFalse(s.active)
        s = s.toggle("a1")
        assertTrue(s.active)
        assertEquals(setOf("a1"), s.rows)
        s = s.toggle("a1")
        assertTrue(s.isEmpty)
        // Unpicking the last row does not leave selecting: that is "done".
        assertTrue(s.active)
        assertFalse(s.clear().active)
    }

    @Test
    fun `a picked heading and a picked track inside it count the track once`() {
        val s = QueueSelection().toggle("album:a1").toggle("a2")
        assertEquals(listOf("a1", "a2", "a3"), s.itemIds(lines))
    }

    @Test
    fun `select all picks every row`() {
        assertEquals(lines.map { it.id }.toSet(), QueueSelection().all(lines).rows)
    }

    @Test
    fun `rows that left the list are no longer picked`() {
        val s = QueueSelection().toggle("a1").toggle("gone")
        assertEquals(setOf("a1"), s.retain(lines).rows)
        // Unchanged selections are returned as they were.
        val whole = QueueSelection().toggle("a1")
        assertTrue(whole === whole.retain(lines))
    }
}

package cc.blit.koan.ui.queue

import cc.blit.koan.ffi.EntryStatus
import cc.blit.koan.ffi.QueueItem
import org.junit.Assert.assertEquals
import org.junit.Test

class QueueRowsTest {
    private fun item(id: String, album: String, title: String = "t$id", artist: String = "A", ms: Long? = 60_000) = QueueItem(
        queueItemId = id,
        trackId = id.hashCode().toLong(),
        albumId = null,
        artistId = null,
        albumArtistId = null,
        title = title,
        artist = artist,
        albumArtist = artist,
        artistCredit = emptyList(),
        albumArtistCredit = emptyList(),
        creditDiffers = false,
        album = album,
        year = "2007",
        codec = "flac",
        trackNumber = null,
        disc = null,
        durationMs = ms?.toULong(),
        status = EntryStatus.QUEUED,
        playlistEntryId = null,
        failureReason = null,
        onServer = false,
        onDisk = true,
    )

    private val queue = listOf(
        item("1", "X"),
        item("2", "X"),
        item("3", ""),
        item("4", "Y", title = "Y"),
        item("5", "X"),
    )

    @Test
    fun groupsContiguousRunsOnly() {
        val rows = QueueRow.build(queue, grouped = true)
        assertEquals(listOf("album:1", "1", "2", "3", "4", "album:5", "5"), rows.map { it.key })
        assertEquals(QueueRow.Single::class, rows[4]::class)
        assertEquals(listOf("1", "2"), rows[0].itemIds)
    }

    @Test
    fun flatIsOneRowPerTrack() {
        assertEquals(queue.map { it.queueItemId }, QueueRow.build(queue, grouped = false).map { it.key })
    }

    @Test
    fun headingStandsForItsRun() {
        val rows = QueueRow.build(queue, grouped = true)
        assertEquals(listOf("1", "2", "4"), QueueRow.itemIds(setOf("album:1", "2", "4"), rows))
    }

    @Test
    fun dropAnchorsToTheRowBelow() {
        val rows = QueueRow.build(queue, grouped = false)
        // 4 dragged to the top.
        val dropped = listOf(rows[3], rows[0], rows[1], rows[2], rows[4])
        assertEquals("1" to false, moveTarget(dropped, "4", setOf("4"), queue))
    }

    @Test
    fun dropAtTheEndGoesAfterTheLastThatStays() {
        val rows = QueueRow.build(queue, grouped = false)
        val dropped = rows.drop(1) + rows[0]
        assertEquals("5" to true, moveTarget(dropped, "1", setOf("1"), queue))
    }

    @Test
    fun headingMovesItsRun() {
        val rows = QueueRow.build(queue, grouped = true)
        val run = setOf("1", "2")
        // The run's tracks leave the list while the heading moves; dropped below "3".
        val lifted = rows.filterNot { it is QueueRow.Track && it.item.queueItemId in run }
        val dropped = listOf(lifted[1], lifted[0]) + lifted.drop(2)
        assertEquals("4" to false, moveTarget(dropped, "album:1", run, queue))
    }

    @Test
    fun groupDetail() {
        assertEquals("2007 · 2 tracks · 2:00 · FLAC", QueueGroup("A", "X", queue.take(2)).detail)
        assertEquals("1:01:01", duration(3_661_000))
    }
}

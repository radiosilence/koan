package cc.blit.koan.ui

import androidx.compose.ui.unit.dp
import cc.blit.koan.album
import cc.blit.koan.item
import cc.blit.koan.state.AlbumRecord
import cc.blit.koan.state.ArtistRecord
import cc.blit.koan.track
import cc.blit.koan.artist
import cc.blit.koan.ui.pages.Availability
import cc.blit.koan.ui.pages.AlbumView
import cc.blit.koan.ui.pages.ArtistView
import cc.blit.koan.ui.pages.BioState
import cc.blit.koan.ui.pages.Pick
import cc.blit.koan.ui.pages.TrackLine
import cc.blit.koan.ui.pages.albumDetails
import cc.blit.koan.ui.pages.albumView
import cc.blit.koan.ui.pages.artistView
import cc.blit.koan.ui.pages.availability
import cc.blit.koan.ui.pages.bioState
import cc.blit.koan.ui.pages.countOf
import cc.blit.koan.ui.pages.displayTitle
import cc.blit.koan.ui.pages.formatDuration
import cc.blit.koan.ui.pages.gridColumns
import cc.blit.koan.ui.pages.hasPhoto
import cc.blit.koan.ui.pages.highlightIndex
import cc.blit.koan.ui.pages.kilohertz
import cc.blit.koan.ui.pages.tileWidth
import cc.blit.koan.ui.pages.trackLines
import cc.blit.koan.ui.pages.trackQuality
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.ArtistInfo
import uniffi.koan_ffi.EntryStatus

class RecordSupportTest {
    private fun onDisc(id: Long, disc: Int?) = track(id).copy(disc = disc)

    @Test
    fun `a record on one disc has no disc headings`() {
        val lines = trackLines(listOf(track(1), track(2), track(3)))
        assertEquals(3, lines.size)
        assertTrue(lines.all { it is TrackLine.Row })
        assertEquals(listOf(0, 1, 2), lines.map { (it as TrackLine.Row).position })
    }

    @Test
    fun `a record with no disc numbers is one disc`() {
        assertTrue(trackLines(listOf(onDisc(1, null), onDisc(2, null))).none { it is TrackLine.Disc })
        assertTrue(trackLines(listOf(onDisc(1, null), onDisc(2, 1))).none { it is TrackLine.Disc })
    }

    @Test
    fun `a record on several discs is headed where each disc begins and keeps the engine's order`() {
        val lines = trackLines(listOf(onDisc(1, 1), onDisc(2, 1), onDisc(3, 2), onDisc(4, 2), onDisc(5, 3)))
        assertEquals(
            listOf("D1", "t1", "t2", "D2", "t3", "t4", "D3", "t5"),
            lines.map { if (it is TrackLine.Disc) "D${it.disc}" else "t${(it as TrackLine.Row).track.id}" },
        )
        // Positions count tracks only: playing the fifth starts at index 4, not 7.
        assertEquals(4, (lines.last() as TrackLine.Row).position)
        // A heading is not regrouped: a disc that comes back is headed again, with a key of its own.
        val back = trackLines(listOf(onDisc(1, 1), onDisc(2, 2), onDisc(3, 1)))
        assertEquals(listOf(1, 2, 1), back.filterIsInstance<TrackLine.Disc>().map { it.disc })
        assertEquals(back.map { it.key }.toSet().size, back.size)
    }

    @Test
    fun `line keys are stable across a reload that changes nothing`() {
        val tracks = listOf(onDisc(1, 1), onDisc(2, 2))
        assertEquals(trackLines(tracks).map { it.key }, trackLines(tracks.map { it.copy() }).map { it.key })
    }

    @Test
    fun `the highlighted track is found counting the header, and only on the record that holds it`() {
        val lines = trackLines(listOf(onDisc(1, 1), onDisc(2, 2), onDisc(3, 2)))
        // header(0), D1(1), t1(2), D2(3), t2(4), t3(5)
        assertEquals(2, highlightIndex(lines, 1))
        assertEquals(5, highlightIndex(lines, 3))
        assertEquals(4, highlightIndex(lines, 3, leading = 0))
        assertNull(highlightIndex(lines, 99))
        assertNull(highlightIndex(lines, null))
    }

    @Test
    fun `an album view says loading, failed, gone or ready`() {
        assertEquals(AlbumView.Loading, albumView(null))
        assertEquals(AlbumView.Failed, albumView(AlbumRecord(1, 0, null, emptyList(), failed = true)))
        assertEquals(AlbumView.Gone, albumView(AlbumRecord(1, 0, null, emptyList())))
        val ready = albumView(AlbumRecord(1, 0, album(1), listOf(track(1))))
        assertTrue(ready is AlbumView.Ready && ready.tracks.size == 1)
    }

    @Test
    fun `details are year, codec and total length`() {
        assertEquals("2000 · FLAC · 6:00", albumDetails(album(1), listOf(track(1), track(2))))
        assertEquals("2000 · FLAC", albumDetails(album(1), listOf(track(1).copy(durationMs = null))))
    }

    @Test
    fun `durations and formats read as the Swift app's do`() {
        assertEquals("--:--", formatDuration(null))
        assertEquals("--:--", formatDuration(0))
        assertEquals("3:05", formatDuration(185_000))
        assertEquals("1:01:01", formatDuration(3_661_000))
        assertEquals("44.1", kilohertz(44100))
        assertEquals("96", kilohertz(96000))
        assertEquals("FLAC 16/44.1", trackQuality(track(1)))
        assertEquals("MP3 44.1 kHz", trackQuality(track(1).copy(codec = "mp3", bitDepth = null)))
        assertNull(trackQuality(track(1).copy(codec = null)))
        assertEquals("1 album", countOf(1, "album"))
        assertEquals("3 albums", countOf(3, "album"))
        assertEquals("Set", displayTitle("2019-04-02: Set"))
        assertEquals("2019-04-02: ", displayTitle("2019-04-02: "))
        assertEquals("Plain", displayTitle("Plain"))
    }

    @Test
    fun `what the queue is doing about a track wins over what the library knows`() {
        val remote = track(1).copy(onServer = true, onDisk = false)
        assertEquals(Availability.Cloud, availability(remote, null))
        assertEquals(Availability.Kept, availability(remote.copy(onDisk = true), null))
        assertEquals(Availability.Local, availability(track(1), null))
        assertEquals(Availability.None, availability(track(1).copy(onDisk = false), null))
        assertEquals(Availability.Waiting, availability(remote, item("a", 1, status = EntryStatus.PRIORITY_PENDING)))
        assertEquals(Availability.Downloading, availability(remote, item("a", 1, status = EntryStatus.DOWNLOADING)))
        assertEquals(Availability.Failed, availability(remote, item("a", 1, status = EntryStatus.FAILED)))
        // Queued and playing say nothing the badge cannot.
        assertEquals(Availability.Cloud, availability(remote, item("a", 1, status = EntryStatus.QUEUED)))
    }

    @Test
    fun `a pick is a mode, held in the order things were ticked`() {
        var pick = Pick<Long>()
        assertFalse(pick.active)
        pick = pick.begin(3)
        assertTrue(pick.active)
        pick = pick.toggle(1).toggle(2)
        assertEquals(listOf(3L, 1L, 2L), pick.picked)
        assertTrue(1L in pick)
        pick = pick.toggle(1)
        assertEquals(listOf(3L, 2L), pick.picked)
        assertFalse(1L in pick)
        // Select all adds what is missing, in page order, after what was ticked.
        pick = pick.selectAll(listOf(1, 2, 3, 4))
        assertEquals(listOf(3L, 2L, 1L, 4L), pick.picked)
        assertEquals(4, pick.count)
        // Un-ticking the last one leaves the mode on; ending drops everything.
        assertTrue(Pick<Long>().begin().active)
        assertEquals(Pick<Long>(), pick.end())
    }

    @Test
    fun `the artist page is guarded on the id and chooses between records and appearances`() {
        val owned = ArtistRecord(5, 0, artist(5), listOf(album(1)))
        assertEquals(ArtistView.Loading, artistView(null, 5))
        assertEquals(ArtistView.Loading, artistView(owned, 6))
        assertEquals(ArtistView.Discography(listOf(album(1))), artistView(owned, 5))

        val guest = ArtistRecord(5, 0, artist(5), emptyList(), appearances = listOf(track(1, album = 7), track(2, album = 7), track(3, album = 8)))
        val view = artistView(guest, 5)
        assertTrue(view is ArtistView.Appearances && view.albumCount == 2 && view.tracks.size == 3)

        assertEquals(ArtistView.Missing, artistView(ArtistRecord(5, 0, null, emptyList()), 5))
        assertEquals(ArtistView.Discography(emptyList()), artistView(ArtistRecord(5, 0, artist(5), emptyList()), 5))
    }

    @Test
    fun `the biography shows text, a lookup in progress, or nothing`() {
        fun record(info: ArtistInfo?, loading: Boolean) = ArtistRecord(5, 0, artist(5), emptyList(), info = info, infoLoading = loading)
        assertEquals(BioState.Hidden, bioState(null))
        assertEquals(BioState.Hidden, bioState(record(null, false)))
        assertEquals(BioState.Looking, bioState(record(null, true)))
        val text = bioState(record(ArtistInfo("One.\nTwo.\n\n", "https://en.wikipedia.org/x", true, "Someone CC-BY"), false))
        assertEquals(BioState.Text(listOf("One.", "Two."), "https://en.wikipedia.org/x", "Someone CC-BY"), text)
        // A cached photograph with no article yet is not a biography, but a lookup may be under way.
        assertEquals(BioState.Looking, bioState(record(ArtistInfo(null, null, true, null), true)))
        assertEquals(BioState.Hidden, bioState(record(ArtistInfo("  ", null, false, null), false)))
    }

    @Test
    fun `the photograph is drawn only when there is one`() {
        assertFalse(hasPhoto(null))
        assertTrue(hasPhoto(ArtistRecord(5, 0, artist(5), emptyList(), info = ArtistInfo(null, null, true, null))))
        assertFalse(hasPhoto(ArtistRecord(5, 0, artist(5), emptyList(), info = ArtistInfo("x", null, false, null))))
    }

    @Test
    fun `the cover is as wide as one column of the grid`() {
        assertEquals(2, gridColumns(360.dp))
        assertEquals(2, gridColumns(411.dp))
        assertEquals(3, gridColumns(600.dp))
        assertEquals(5, gridColumns(1000.dp))
        // 360 wide, 22 margins, 16 gutter: (360 - 44 - 16) / 2.
        assertEquals(150.dp, tileWidth(360.dp, 2))
        // The columns and the gaps fill the width exactly.
        val w = 1000.dp
        val n = gridColumns(w)
        assertEquals(w.value, (tileWidth(w, n) * n + 16.dp * (n - 1) + 44.dp).value, 0.001f)
    }
}

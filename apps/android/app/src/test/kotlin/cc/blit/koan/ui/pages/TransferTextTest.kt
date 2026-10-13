package cc.blit.koan.ui.pages

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.koan_ffi.Transfer
import uniffi.koan_ffi.TransferFigure
import uniffi.koan_ffi.TransferState

class TransferTextTest {
    private fun transfer(state: TransferState, artist: String = "Burial", reason: String? = null) =
        Transfer(trackId = 7, albumId = 3, title = "Archangel", artist = artist, state = state, failureReason = reason)

    private fun figure(progress: Double?, written: ULong = 0uL, total: ULong = 0uL, rate: ULong = 0uL) =
        TransferFigure(trackId = 7, progress = progress, bytesWritten = written, totalBytes = total, bytesPerSecond = rate)

    @Test
    fun `a landed transfer is whole and one with no length is empty not full`() {
        assertEquals(1f, TransferText.fraction(TransferState.DONE, null), 0f)
        assertEquals(0f, TransferText.fraction(TransferState.RUNNING, figure(null)), 0f)
        assertEquals(0.42f, TransferText.fraction(TransferState.RUNNING, figure(0.42)), 1e-6f)
        assertEquals(1f, TransferText.fraction(TransferState.RUNNING, figure(1.7)), 0f)
    }

    @Test
    fun `the trailing word is the percent while running and the state otherwise`() {
        assertEquals("42%", TransferText.figure(transfer(TransferState.RUNNING), figure(0.425)))
        assertEquals("", TransferText.figure(transfer(TransferState.RUNNING), figure(null)))
        assertEquals("Queued", TransferText.figure(transfer(TransferState.QUEUED), null))
        assertEquals("Done", TransferText.figure(transfer(TransferState.DONE), null))
        assertEquals("Failed", TransferText.figure(transfer(TransferState.FAILED), null))
    }

    @Test
    fun `a running subtitle gives rate and size, and a rate of nothing is a stall`() {
        val moving = figure(0.5, written = 210_000_000uL, total = 451_000_000uL, rate = 4_200_000uL)
        assertEquals("Burial · 4.2 MB/s · 210 MB of 451 MB", TransferText.subtitle(transfer(TransferState.RUNNING), moving))
        val stalled = figure(0.5, written = 210_000_000uL, total = 451_000_000uL, rate = 0uL)
        assertEquals("Burial · stalled · 210 MB of 451 MB", TransferText.subtitle(transfer(TransferState.RUNNING), stalled))
        assertEquals("stalled", TransferText.subtitle(transfer(TransferState.RUNNING, artist = ""), null))
        assertEquals("Burial · 1.0 MB/s · 5.0 MB", TransferText.subtitle(transfer(TransferState.RUNNING), figure(null, written = 5_000_000uL, rate = 1_000_000uL)))
    }

    @Test
    fun `settled and waiting subtitles`() {
        assertEquals("Couldn't be fetched", TransferText.subtitle(transfer(TransferState.FAILED), null))
        assertEquals("404 Not Found", TransferText.subtitle(transfer(TransferState.FAILED, reason = "404 Not Found"), null))
        assertEquals("Burial — waiting", TransferText.subtitle(transfer(TransferState.QUEUED), null))
        assertEquals("Waiting", TransferText.subtitle(transfer(TransferState.QUEUED, artist = ""), null))
        assertEquals("Burial — downloaded", TransferText.subtitle(transfer(TransferState.DONE), null))
        assertEquals("Downloaded", TransferText.subtitle(transfer(TransferState.DONE, artist = ""), null))
    }

    @Test
    fun `only queued and running transfers follow the fast stream`() {
        assertEquals(listOf(true, true, false, false), listOf(TransferState.QUEUED, TransferState.RUNNING, TransferState.DONE, TransferState.FAILED).map(TransferText::isLive))
    }

    @Test
    fun `byte sizes and task counts`() {
        assertEquals("0 B", TransferText.bytes(0))
        assertEquals("999 B", TransferText.bytes(999))
        assertEquals("1.5 KB", TransferText.bytes(1_500))
        assertEquals("812 KB", TransferText.bytes(812_000))
        assertEquals("3.2 GB", TransferText.bytes(3_200_000_000))
        assertEquals("12,400 of 49,700 tracks", TransferText.counts(12_400, 49_700, "tracks"))
        assertEquals("12,345 / 48,087", TransferText.counts(12_345, 48_087, null))
        assertEquals("30 albums", TransferText.counts(30, null, "albums"))
        assertEquals(null, TransferText.counts(null, 10, "tracks"))
    }
}

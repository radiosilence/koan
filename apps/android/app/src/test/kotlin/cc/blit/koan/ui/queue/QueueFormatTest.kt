package cc.blit.koan.ui.queue

import cc.blit.koan.format
import cc.blit.koan.item
import cc.blit.koan.service.BitPerfectStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.DspInfo

class QueueFormatTest {
    @Test
    fun `durations are m ss, and h mm ss once they earn the hour`() {
        assertEquals("3:05", QueueFormat.duration(185_000))
        assertEquals("1:02:03", QueueFormat.duration(3_723_000))
        assertEquals("--:--", QueueFormat.duration(null))
        assertEquals("--:--", QueueFormat.duration(0))
        assertEquals("0:00", QueueFormat.clock(0))
    }

    @Test
    fun `a date prefix is dropped from a title unless nothing would be left`() {
        assertEquals("BBC Essential Mix", QueueFormat.title("2022-03-12: BBC Essential Mix"))
        assertEquals("Plain", QueueFormat.title("Plain"))
        assertEquals("2022-03-12:", QueueFormat.title("2022-03-12:"))
    }

    @Test
    fun `rates drop a trailing point zero`() {
        assertEquals("44.1", QueueFormat.rate(44_100))
        assertEquals("96", QueueFormat.rate(96_000))
    }

    @Test
    fun `quality names the source, and a rate the device refused`() {
        val f = format(96_000u)
        assertEquals("FLAC 16/96", QueueFormat.quality(f))
        assertEquals("FLAC 16/96 → 48", QueueFormat.quality(f.copy(outputSampleRate = 48_000u)))
        assertEquals("FLAC 16/96", QueueFormat.quality(f.copy(outputSampleRate = 96_000u)))
        assertEquals("FLAC 16/96 6ch", QueueFormat.quality(f.copy(channels = 6u.toUShort())))
    }

    @Test
    fun `quality names dsp so the badge does not claim an untouched path`() {
        val f = format(44_100u)
        assertTrue(QueueFormat.quality(f.copy(dsp = DspInfo("p", true, null))).endsWith("· EQ"))
        assertTrue(QueueFormat.quality(f.copy(dsp = DspInfo("p", true, 48_000u))).endsWith("· EQ + FIR"))
        assertTrue(QueueFormat.quality(f.copy(dsp = DspInfo("p", false, 48_000u))).endsWith("· FIR"))
    }

    @Test
    fun `the explanation says whether anything resampled`() {
        val f = format(96_000u)
        assertTrue(QueueFormat.outputExplanation(f).startsWith("Source format"))
        assertTrue(QueueFormat.outputExplanation(f.copy(outputSampleRate = 96_000u)).contains("resampling nothing"))
        assertTrue(QueueFormat.outputExplanation(f.copy(outputSampleRate = 48_000u)).contains("being resampled"))
    }

    @Test
    fun `a group's detail names the year, count, length and a shared codec`() {
        val items = listOf(item("1").copy(year = "2007"), item("2").copy(year = "2007"))
        val group = QueueGroup("1", "Artist", "Album", items)
        assertEquals("2007 · 2 tracks · 6:00 · FLAC", QueueFormat.groupDetail(group))
        val mixed = group.copy(items = listOf(items[0], items[1].copy(codec = "MP3")))
        assertEquals("2007 · 2 tracks · 6:00", QueueFormat.groupDetail(mixed))
    }

    @Test
    fun `the queue summary counts tracks and totals their length`() {
        assertEquals("1 track · 3:00", QueueFormat.summary(listOf(item("1"))))
        assertEquals("0 tracks", QueueFormat.summary(emptyList()))
    }

    @Test
    fun `the badge's verdict says why a route is not bit-perfect`() {
        assertEquals("bit-perfect · exclusive to the DAC", BadgeText.verdict(BitPerfectStatus.Exclusive))
        assertTrue(BadgeText.verdict(BitPerfectStatus.MixerBitPerfect).startsWith("bit-perfect"))
        assertEquals(
            "not bit-perfect · only a USB DAC can be bit-perfect",
            BadgeText.verdict(BitPerfectStatus.No("Only a USB DAC can be bit-perfect")),
        )
    }
}

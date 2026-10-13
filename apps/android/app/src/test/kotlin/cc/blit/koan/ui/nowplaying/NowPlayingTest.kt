package cc.blit.koan.ui.nowplaying

import cc.blit.koan.ffi.DspInfo
import cc.blit.koan.ffi.LyricLine
import cc.blit.koan.ffi.RepeatMode
import cc.blit.koan.ffi.StreamFormat
import org.junit.Assert.assertEquals
import org.junit.Test

class NowPlayingTest {
    private fun format(rate: UInt, depth: UShort?, out: UInt? = null, dsp: DspInfo? = null, codec: String = "flac") =
        StreamFormat(codec, rate, depth, null, 2u, out, dsp)

    @Test
    fun qualityBadge() {
        assertEquals("FLAC 24/96", quality(format(96_000u, 24u)))
        assertEquals("FLAC 16/44.1", quality(format(44_100u, 16u, out = 44_100u)))
        assertEquals("MP3 44.1 kHz", quality(format(44_100u, null, codec = "mp3")))
        assertEquals("FLAC 16/44.1 → 48 · EQ", quality(format(44_100u, 16u, out = 48_000u, dsp = DspInfo("p", true, null))))
    }

    @Test
    fun repeatSteps() {
        assertEquals(RepeatMode.QUEUE, nextRepeat(RepeatMode.OFF))
        assertEquals(RepeatMode.ONE, nextRepeat(RepeatMode.QUEUE))
        assertEquals(RepeatMode.OFF, nextRepeat(RepeatMode.ONE))
    }

    @Test
    fun lyricLineAtPosition() {
        val lines = listOf(LyricLine(1.0, "a"), LyricLine(5.0, "b"), LyricLine(9.0, "c"))
        assertEquals(-1, lineAt(lines, 0.5))
        assertEquals(1, lineAt(lines, 5.0))
        assertEquals(2, lineAt(lines, 60.0))
    }

    @Test
    fun clockRunsPastAnHour() {
        assertEquals("3:05", clock(185_000))
        assertEquals("1:00:00", clock(3_600_000))
    }
}

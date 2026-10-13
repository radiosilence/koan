package cc.blit.koan.ui.nowplaying

import cc.blit.koan.ffi.DspInfo
import cc.blit.koan.ffi.LyricLine
import cc.blit.koan.ffi.RepeatMode
import cc.blit.koan.ffi.StreamFormat
import org.junit.Assert.assertEquals
import org.junit.Test

class NowPlayingTest {
    private fun format(rate: UInt, depth: UShort?, out: UInt? = null, dsp: DspInfo? = null, codec: String = "flac", mixed: Boolean? = null) =
        StreamFormat(codec, rate, depth, null, 2u, out, mixed, dsp)

    @Test
    fun qualityBadge() {
        assertEquals("FLAC 24/96", quality(format(96_000u, 24u)))
        assertEquals("FLAC 16/44.1", quality(format(44_100u, 16u, out = 44_100u)))
        assertEquals("MP3 44.1 kHz", quality(format(44_100u, null, codec = "mp3")))
        assertEquals("FLAC 16/44.1 → 48 · EQ", quality(format(44_100u, 16u, out = 48_000u, dsp = DspInfo("p", true, null))))
    }

    @Test
    fun theMixerIsNamed() {
        val held = format(96_000u, 24u, out = 96_000u, mixed = false)
        assertEquals("FLAC 24/96" to true, quality(held) to bitPerfect(held))
        val resampled = format(44_100u, 24u, out = 48_000u, mixed = true)
        assertEquals("FLAC 24/44.1 → 48" to false, quality(resampled) to bitPerfect(resampled))
        val mixed = format(48_000u, 16u, out = 48_000u, mixed = true)
        assertEquals("FLAC 16/48 · mixed" to false, quality(mixed) to bitPerfect(mixed))
        val processed = format(48_000u, 16u, out = 48_000u, mixed = false, dsp = DspInfo("p", true, null))
        assertEquals("FLAC 16/48 · EQ" to false, quality(processed) to bitPerfect(processed))
        assertEquals(false, bitPerfect(format(48_000u, 16u, out = 48_000u)))
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

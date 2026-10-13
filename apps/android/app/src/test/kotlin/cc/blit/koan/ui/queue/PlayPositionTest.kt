package cc.blit.koan.ui.queue

import cc.blit.koan.state.Playhead
import org.junit.Assert.assertEquals
import org.junit.Test

class PlayPositionTest {
    private val playing = Playhead(positionMs = 10_000, playing = true, atNanos = 0)

    @Test
    fun `the playhead runs on from its anchor and stops at the track's end`() {
        assertEquals(12_000, PlayPosition.positionMs(playing, null, 60_000, 2_000_000_000))
        assertEquals(60_000, PlayPosition.positionMs(playing, null, 60_000, 90_000_000_000))
    }

    @Test
    fun `a paused playhead stays put`() {
        assertEquals(10_000, PlayPosition.positionMs(playing.copy(playing = false), null, 60_000, 5_000_000_000))
    }

    @Test
    fun `a held thumb wins over the engine`() {
        assertEquals(30_000, PlayPosition.positionMs(playing, 0.5, 60_000, 2_000_000_000))
    }

    @Test
    fun `fraction is clamped and zero for an unknown length`() {
        assertEquals(0.25f, PlayPosition.fraction(15_000, 60_000), 0.0001f)
        assertEquals(1f, PlayPosition.fraction(90_000, 60_000), 0f)
        assertEquals(0f, PlayPosition.fraction(1_000, 0), 0f)
    }
}

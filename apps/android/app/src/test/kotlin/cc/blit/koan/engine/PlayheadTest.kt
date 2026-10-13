package cc.blit.koan.engine

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlayheadTest {
    private val anchor = Playhead(positionMs = 18_000, seekableMs = 20_000, playing = true, atMs = 1_000)

    @Test
    fun countsOnButNotPastTheTrack() {
        assertEquals(19_500L, anchor.positionAt(nowMs = 2_500, durationMs = 20_000))
        assertEquals(20_000L, anchor.positionAt(nowMs = 4_473, durationMs = 20_000))
        assertEquals(21_473L, anchor.positionAt(nowMs = 4_473))
    }

    @Test
    fun aDriftedAnchorRestatesTheLast() {
        assertTrue(Playhead(21_040, 20_000, true, 4_000).continues(anchor))
        assertFalse(Playhead(5_000, 20_000, true, 4_000).continues(anchor))
        assertFalse(Playhead(21_000, 20_000, false, 4_000).continues(anchor))
    }
}

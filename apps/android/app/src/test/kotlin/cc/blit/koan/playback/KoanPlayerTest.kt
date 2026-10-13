package cc.blit.koan.playback

import androidx.media3.common.Player
import cc.blit.koan.ffi.RepeatMode
import org.junit.Assert.assertEquals
import org.junit.Test

class KoanPlayerTest {
    @Test
    fun repeatRoundTrips() {
        for (mode in RepeatMode.entries) assertEquals(mode, koanRepeat(mediaRepeat(mode)))
        assertEquals(Player.REPEAT_MODE_ALL, mediaRepeat(RepeatMode.QUEUE))
    }

}

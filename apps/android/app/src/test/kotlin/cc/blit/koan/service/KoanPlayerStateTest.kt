package cc.blit.koan.service

import androidx.media3.common.Player
import cc.blit.koan.item
import cc.blit.koan.nowPlaying
import cc.blit.koan.state.Playhead
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.RepeatMode

class KoanPlayerStateTest {
    private fun snapshot(
        state: PlayState,
        entry: Boolean = true,
        waiting: Boolean = false,
        seekableMs: Long = 180_000,
        queue: Int = 3,
        head: Playhead = Playhead(30_000, false, 0),
        repeat: RepeatMode = RepeatMode.OFF,
        shuffle: Boolean = false,
    ) = PlayerSnapshot(
        nowPlaying(state, if (entry) item("q1", 1, title = "Song") else null, waiting, repeat = repeat, shuffle = shuffle),
        head, seekableMs, queue,
    )

    @Test
    fun `nothing queued is idle with nothing to play`() {
        val idle = snapshot(PlayState.STOPPED, entry = false, queue = 0)
        val state = KoanPlayerState.map(idle)
        assertEquals(Player.STATE_IDLE, state.playbackState)
        assertTrue(state.playlist.isEmpty())
        assertFalse(state.playWhenReady)
        assertFalse(KoanPlayerState.commandIds(idle).contains(Player.COMMAND_PLAY_PAUSE))
        assertTrue(KoanPlayerState.commandIds(idle).contains(Player.COMMAND_SET_SHUFFLE_MODE))
    }

    @Test
    fun `playing is ready with play-when-ready and one item that carries the tags`() {
        val state = KoanPlayerState.map(snapshot(PlayState.PLAYING))
        assertEquals(Player.STATE_READY, state.playbackState)
        assertTrue(state.playWhenReady)
        assertEquals(1, state.playlist.size)
        assertEquals(0, state.currentMediaItemIndex)
        val item = state.playlist[0]
        assertEquals("q1", item.uid)
        assertEquals("Song", item.mediaMetadata!!.title.toString())
        assertEquals("Artist", item.mediaMetadata!!.artist.toString())
        assertEquals(180_000_000L, item.durationUs)
    }

    @Test
    fun `paused and stopped-with-a-track are ready but not playing`() {
        for (s in listOf(PlayState.PAUSED, PlayState.STOPPED)) {
            val state = KoanPlayerState.map(snapshot(s))
            assertEquals(Player.STATE_READY, state.playbackState)
            assertFalse(state.playWhenReady)
        }
    }

    @Test
    fun `waiting for the track to arrive is buffering and wants to play`() {
        val state = KoanPlayerState.map(snapshot(PlayState.STOPPED, waiting = true))
        assertEquals(Player.STATE_BUFFERING, state.playbackState)
        assertTrue(state.playWhenReady)
    }

    @Test
    fun `the position runs on from the anchor without being pushed`() {
        val anchor = Playhead(30_000, playing = true, atNanos = System.nanoTime() - 5_000_000_000L)
        val state = KoanPlayerState.map(snapshot(PlayState.PLAYING, head = anchor))
        val position = state.contentPositionMsSupplier.get()
        assertTrue("position $position", position in 35_000..36_500)
        val paused = KoanPlayerState.map(snapshot(PlayState.PAUSED, head = Playhead(30_000, false, 0)))
        assertEquals(30_000L, paused.contentPositionMsSupplier.get())
    }

    @Test
    fun `seeking is offered only once there is something to seek`() {
        val seekable = KoanPlayerState.commandIds(snapshot(PlayState.PLAYING))
        assertTrue(seekable.contains(Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM))
        val notYet = KoanPlayerState.commandIds(snapshot(PlayState.PLAYING, seekableMs = 0))
        assertFalse(notYet.contains(Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM))
        assertTrue(notYet.contains(Player.COMMAND_SEEK_TO_NEXT))
        assertTrue(notYet.contains(Player.COMMAND_SEEK_TO_PREVIOUS))
    }

    @Test
    fun `modes map both ways`() {
        val state = KoanPlayerState.map(snapshot(PlayState.PLAYING, repeat = RepeatMode.ONE, shuffle = true))
        assertEquals(Player.REPEAT_MODE_ONE, state.repeatMode)
        assertTrue(state.shuffleModeEnabled)
        for (mode in RepeatMode.entries) {
            assertEquals(mode, KoanPlayerState.repeat(KoanPlayerState.repeat(mode)))
        }
        assertEquals(RepeatMode.QUEUE, KoanPlayerState.repeat(Player.REPEAT_MODE_ALL))
    }
}

package cc.blit.koan.state

import cc.blit.koan.album
import cc.blit.koan.track
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlayableSelectionTest {
    private val artist = Playable.OfArtist(1, "Artist 1")
    private val record = Playable.OfAlbum(album(1))
    private val song = Playable.OfTrack(track(1))

    @Test
    fun `ids collide across kinds, picks do not`() {
        val pick = PlayableSelection()
        pick.begin(artist)
        pick.toggle(record)
        pick.toggle(song)
        assertEquals(listOf(artist, record, song), pick.pick.value.picked)
        assertTrue(record.key in pick.pick.value)
        pick.toggle(record)
        assertEquals(listOf(artist, song), pick.pick.value.picked)
        assertFalse(record.key in pick.pick.value)
    }

    @Test
    fun `held in the order ticked, not the page's`() {
        val pick = PlayableSelection { listOf(artist, record, song) }
        pick.begin(song)
        pick.toggle(artist)
        assertEquals(listOf(song, artist), pick.pick.value.picked)
        // Select all adds what is missing after what was ticked.
        pick.selectAll()
        assertEquals(listOf(song, artist, record), pick.pick.value.picked)
    }

    @Test
    fun `a tap is taken only while picking`() {
        val pick = PlayableSelection()
        assertFalse("outside the mode a tap opens", pick.take(song))
        assertTrue(pick.pick.value.picked.isEmpty())
        pick.begin()
        assertTrue(pick.take(song))
        assertEquals(listOf(song), pick.pick.value.picked)
    }

    @Test
    fun `finishing hands back the pick and puts the page back`() {
        val pick = PlayableSelection()
        pick.begin(record)
        assertEquals(listOf(record), pick.finish())
        assertFalse(pick.isActive)
        assertTrue(pick.pick.value.picked.isEmpty())
        pick.begin(record)
        pick.end()
        assertEquals(Pick(), pick.pick.value)
    }
}

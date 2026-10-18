package cc.blit.koan.ui.library

import android.app.Application
import android.content.Intent
import cc.blit.koan.ffi.Share
import cc.blit.koan.ffi.Track
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class RecordPageTest {
    private fun track(id: Long, disc: Int?, number: Int) = Track(
        id, "Track $id", "Artist", emptyList(), "Artist", "Record", 1, 1, disc, number, 180_000, "FLAC",
        44_100, 16, 2, null, null, "remote", true, false, null, false,
    )

    @Test
    fun oneDiscHasNoDiscHeadings() {
        val items = recordItems(listOf(track(1, 1, 1), track(2, 1, 2)))
        assertEquals(listOf(RecordItem.Heading, RecordItem.Line(0, track(1, 1, 1)), RecordItem.Line(1, track(2, 1, 2))), items)
    }

    @Test
    fun aTrackOpenedFromElsewhereIsFoundUnderItsDisc() {
        val tracks = listOf(track(1, 1, 1), track(2, 1, 2), track(3, 2, 1), track(4, 2, 2))
        val items = recordItems(tracks)
        assertEquals(RecordItem.Disc(2), items[4])
        val at = items.indexOfFirst { it is RecordItem.Line && it.track.id == 4L }
        assertEquals(6, at)
        // Its place in the running order, which is what playing it starts from.
        assertEquals(3, (items[at] as RecordItem.Line).index)
    }

    @Test
    fun aLinkGoesToTheShareSheetWithItsName() {
        val chooser = shareIntent(Share("https://koan.example/s/abc", 12u, 0u), "In Rainbows")
        assertEquals(Intent.ACTION_CHOOSER, chooser.action)
        @Suppress("DEPRECATION")
        val send = chooser.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)!!
        assertEquals(Intent.ACTION_SEND, send.action)
        assertEquals("https://koan.example/s/abc", send.getStringExtra(Intent.EXTRA_TEXT))
        assertEquals("In Rainbows", send.getStringExtra(Intent.EXTRA_SUBJECT))
    }

    @Test
    fun aPartialLinkSaysWhatItLeftOut() {
        assertNull(skippedNote(Share("u", 3u, 0u)))
        assertEquals("2 tracks only on this device left out of the link.", skippedNote(Share("u", 3u, 2u)))
    }
}

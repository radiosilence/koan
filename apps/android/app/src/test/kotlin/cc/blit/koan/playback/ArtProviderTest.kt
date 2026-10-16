package cc.blit.koan.playback

import android.app.Application
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ArtProviderTest {
    @Test
    fun aCoverIsNamedByItsRecordFirst() {
        val uri = ArtProvider.uri("cc.blit.koan.art", 293, 7)
        assertEquals("content://cc.blit.koan.art/album/293", uri.toString())
        assertEquals(Art.album(293, ArtSize.Thumb), ArtProvider.artOf(uri!!))
        assertEquals(Art.track(7, ArtSize.Thumb), ArtProvider.artOf(ArtProvider.uri("cc.blit.koan.art", null, 7)!!))
        assertNull(ArtProvider.uri("cc.blit.koan.art", null, null))
    }

    @Test
    fun nothingElseIsACover() {
        assertNull(ArtProvider.artOf(android.net.Uri.parse("content://cc.blit.koan.files/logs/report.txt")))
        assertNull(ArtProvider.artOf(android.net.Uri.parse("content://cc.blit.koan.art/album/../../databases")))
        assertNull(ArtProvider.artOf(android.net.Uri.parse("koan-art://album/293")))
    }
}

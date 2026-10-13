package cc.blit.koan.ui.pages

import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import cc.blit.koan.theme.KoanTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import uniffi.koan_ffi.Artist
import uniffi.koan_ffi.AlbumSort

/**
 * The browsers' rows and strips, composed on the JVM. They compose, show what
 * the listing gave, and report taps; how they look and scroll is for a device.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = android.app.Application::class)
class BrowseRowsTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun `an artist row shows its counts, keeps the name's case and opens by id`() {
        val opened = mutableListOf<Long>()
        compose.setContent {
            KoanTheme { ArtistRow(Artist(7, "Björk", null, 1, 12), favourite = false, onOpen = { opened += it }) }
        }
        compose.onNodeWithText("1 album · 12 tracks", ignoreCase = true).assertExists()
        compose.onNodeWithText("Björk").performClick()
        assertEquals(listOf(7L), opened)
    }

    @Test
    fun `a choice strip selects what is tapped and any clears`() {
        val chosen = mutableListOf<AlbumSort?>()
        compose.setContent {
            KoanTheme {
                ChoiceStrip(
                    options = listOf(AlbumSort.TITLE to "Title", AlbumSort.YEAR to "Year"),
                    selected = AlbumSort.TITLE, onSelect = { chosen += it }, any = true,
                )
            }
        }
        compose.onNodeWithText("Year").performClick()
        compose.onNodeWithText("any").performClick()
        assertEquals(listOf<AlbumSort?>(AlbumSort.YEAR, null), chosen)
    }
}

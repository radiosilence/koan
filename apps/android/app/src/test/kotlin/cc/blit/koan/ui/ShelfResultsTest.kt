package cc.blit.koan.ui

import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.longClick
import cc.blit.koan.state.Pick
import cc.blit.koan.state.PlayableSelection
import cc.blit.koan.state.ShelfList
import cc.blit.koan.state.ShelfTotals
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.track
import cc.blit.koan.artist
import cc.blit.koan.ui.pages.ShelfActions
import cc.blit.koan.ui.pages.ShelfResults
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The results list composed on the JVM: headings carry the listing's count
 * and open it, a tap goes where it should, a long press starts picking. How
 * it looks is for a device.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = android.app.Application::class)
class ShelfResultsTest {
    @get:Rule
    val compose = createComposeRule()

    private val opened = mutableListOf<ShelfList>()
    private val artists = mutableListOf<Long>()
    private val tapped = mutableListOf<Pair<Long, Int>>()
    private val actions = ShelfActions(
        openSection = { opened += it },
        openArtist = { artists += it },
        openAlbum = {},
        tapTrack = { t, i -> tapped += t.id to i },
        toggleFavourite = {},
    )

    private fun show(selection: PlayableSelection = PlayableSelection()) {
        compose.setContent {
            KoanTheme {
                ShelfResults(
                    artists = listOf(artist(1)),
                    albums = emptyList(),
                    tracks = listOf(track(7, album = null), track(8, album = null)),
                    totals = ShelfTotals(artists = 1, albums = 0, tracks = 240),
                    favouriteTracks = emptySet(),
                    pick = Pick(),
                    selection = selection,
                    actions = actions,
                )
            }
        }
    }

    @Test
    fun `a heading says the listing's count and opens it`() {
        show()
        // 2 tracks shown out of 240.
        compose.onNodeWithText("240").assertExists()
        compose.onNodeWithText("tracks").performClick()
        assertEquals(listOf(ShelfList.Tracks), opened)
    }

    @Test
    fun `taps open an artist and a track, which says where in the list it is`() {
        show()
        compose.onNodeWithText("Artist 1").performClick()
        compose.onNodeWithText("Track 8").performClick()
        assertEquals(listOf(1L), artists)
        assertEquals(listOf(8L to 1), tapped)
    }

    @Test
    fun `a long press starts a pick instead of opening`() {
        val selection = PlayableSelection()
        show(selection)
        compose.onNodeWithText("Track 7").performTouchInput { longClick() }
        compose.waitForIdle()
        assertTrue(selection.isActive)
        assertEquals(1, selection.pick.value.count)
        assertTrue(tapped.isEmpty())
    }
}

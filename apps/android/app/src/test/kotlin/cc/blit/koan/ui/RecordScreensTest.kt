package cc.blit.koan.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.longClick
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanTheme
import cc.blit.koan.track
import cc.blit.koan.ui.pages.Availability
import cc.blit.koan.ui.pages.BarAction
import cc.blit.koan.ui.pages.DiscHeading
import cc.blit.koan.ui.pages.RecordHeader
import cc.blit.koan.ui.pages.RowState
import cc.blit.koan.ui.pages.SelectionBar
import cc.blit.koan.ui.pages.TrackRowView
import cc.blit.koan.ui.pages.numberColumn
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The pieces of the record pages that draw without an engine, composed on the
 * JVM: they compose, show the right words and take taps. How they look is for
 * a device.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = android.app.Application::class)
class RecordScreensTest {
    @get:Rule
    val compose = createComposeRule()

    private fun state(current: Boolean = false, selecting: Boolean = false, picked: Boolean = false, favourite: Boolean = false) =
        RowState(current, favourite, Availability.None, selecting, picked, highlighted = false)

    @Test
    fun `a track row shows its title and length and takes a tap, a long press and the heart`() {
        val log = mutableListOf<String>()
        compose.setContent {
            KoanTheme {
                TrackRowView(
                    track = track(3), position = 3, numberWidth = numberColumn(10), showsAlbum = false, state = state(),
                    onTap = { log += "tap" }, onLongPress = { log += "long" }, onHeart = { log += "heart" },
                )
            }
        }
        compose.onNodeWithText("Track 3").assertExists()
        compose.onNodeWithText("3:00").assertExists()
        compose.onNodeWithText("3").assertExists()
        compose.onNodeWithText("Track 3").performClick()
        compose.onNodeWithText("Track 3").performTouchInput { longClick() }
        compose.onNodeWithContentDescription("Favourite Track 3").performClick()
        assertEquals(listOf("tap", "long", "heart"), log)
    }

    @Test
    fun `a favourite says so, and a pick shows ticks in place of numbers`() {
        compose.setContent {
            KoanTheme {
                Column(Modifier.fillMaxWidth()) {
                    TrackRowView(track(1), 1, numberColumn(10), false, state(favourite = true), {}, {}, {})
                    TrackRowView(track(2), 2, numberColumn(10), false, state(selecting = true, picked = true), {}, {}, {})
                }
            }
        }
        compose.onNodeWithContentDescription("Unfavourite Track 1").assertExists()
        // In a pick the position gives way to the tick: "2" is not drawn, "1" still is.
        compose.onNodeWithText("2").assertDoesNotExist()
    }

    @Test
    fun `the header lays out title, details and actions, and the selection bar counts`() {
        var played = 0
        compose.setContent {
            KoanTheme {
                Column {
                    RecordHeader(
                        title = "Some Record", columns = 2,
                        cover = { _ -> },
                        subtitle = { cc.blit.koan.theme.KoanText("Some Artist", data = true) },
                        actions = { KoanButton("Play", { played++ }) },
                    )
                    DiscHeading(2)
                    SelectionBar(2, listOf(BarAction("Add to queue", {}), BarAction("Done", {}, enabled = false)))
                }
            }
        }
        compose.onNodeWithText("Some Record").assertExists()
        compose.onNodeWithText("Some Artist").assertExists()
        compose.onNodeWithText("disc 2").assertExists()
        compose.onNodeWithText("2 selected").assertExists()
        compose.onNodeWithText("play").performClick()
        assertEquals(1, played)
    }
}

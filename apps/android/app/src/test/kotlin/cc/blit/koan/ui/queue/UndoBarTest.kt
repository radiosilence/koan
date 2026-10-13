package cc.blit.koan.ui.queue

import android.app.Application
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class UndoBarTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun undoTakesTheRemovalBack() {
        var undone = 0
        var timedOut = 0
        compose.setContent { UndoBar(Removal("Engine Dream"), onTimeout = { timedOut++ }) { undone++ } }
        compose.onNodeWithText("Removed Engine Dream").assertExists()
        compose.onNodeWithText("undo").performClick()
        assertEquals(1 to 0, undone to timedOut)
    }

    @Test
    fun itGoesAfterAFewSeconds() {
        var timedOut = 0
        compose.mainClock.autoAdvance = false
        compose.setContent { UndoBar(Removal("Engine Dream"), onTimeout = { timedOut++ }) {} }
        compose.mainClock.advanceTimeBy(4_000)
        assertEquals(0, timedOut)
        compose.mainClock.advanceTimeBy(1_500)
        assertEquals(1, timedOut)
    }
}

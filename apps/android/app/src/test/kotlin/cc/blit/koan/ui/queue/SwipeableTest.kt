package cc.blit.koan.ui.queue

import android.app.Application
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Text
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeLeft
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class SwipeableTest {
    @get:Rule
    val compose = createComposeRule()

    /** Undo puts the row back under its old key; it must come back in place, not swiped away again. */
    @Test
    fun aRowPutBackStays() {
        val rows = mutableStateListOf("Falling Tide", "Engine Dream")
        var removals = 0
        compose.setContent {
            LazyColumn {
                items(rows, key = { it }) { row ->
                    Swipeable(enabled = true, onRemove = {
                        removals++
                        rows.remove(row)
                    }) { Text(row, Modifier.fillMaxWidth().height(56.dp)) }
                }
            }
        }
        compose.onNodeWithText("Falling Tide").performTouchInput { swipeLeft() }
        compose.waitForIdle()
        assertEquals(listOf("Engine Dream"), rows.toList())
        rows.add(0, "Falling Tide")
        compose.waitForIdle()
        compose.onNodeWithText("Falling Tide").assertExists()
        assertEquals(1, removals)
        assertEquals(listOf("Falling Tide", "Engine Dream"), rows.toList())
    }
}

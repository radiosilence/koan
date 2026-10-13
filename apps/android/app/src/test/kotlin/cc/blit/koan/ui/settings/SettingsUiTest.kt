package cc.blit.koan.ui.settings

import android.app.Application
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import cc.blit.koan.ui.theme.KoanTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** Settings' rows, drawn in the kōan theme, which lowercases the app's own words. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class SettingsUiTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun aFailureIsSaidBelowThePage() {
        compose.setContent {
            KoanTheme { SettingsPage("Server", SettingsState(error = "The server refused the password")) { line("Signed in") } }
        }
        compose.onNodeWithText("The server refused the password").assertExists()
    }

    @Test
    fun aChoiceIsPickedFromItsMenu() {
        var chosen = "off"
        compose.setContent {
            var shown by remember { mutableStateOf("off") }
            KoanTheme {
                LazyColumn {
                    item {
                        ChoiceRow("ReplayGain", shown, listOf("Off" to "off", "Per album" to "album")) {
                            shown = it
                            chosen = it
                        }
                    }
                }
            }
        }
        compose.onNodeWithText("off").performClick()
        compose.onNodeWithText("per album").performClick()
        assertEquals("album", chosen)
    }

    @Test
    fun aCodeIsApprovedOnlyOnceTyped() {
        var approved: String? = null
        compose.setContent {
            var code by remember { mutableStateOf("") }
            KoanTheme { InlineField(code, { code = it }, "XXXX-XXXX", "Approve", capitals = true) { approved = code } }
        }
        compose.onNodeWithText("approve").assertIsNotEnabled()
        compose.onNode(hasSetTextAction()).performTextInput("ABCD-EFGH")
        compose.onNodeWithText("approve").assertIsEnabled().performClick()
        assertEquals("ABCD-EFGH", approved)
    }

    @Test
    fun aToggleCommitsItsNewValue() {
        var on = false
        compose.setContent { KoanTheme { SwitchRow("Fade on pause", false) { on = it } } }
        compose.onNodeWithText("fade on pause").performClick()
        assertEquals(true, on)
    }
}

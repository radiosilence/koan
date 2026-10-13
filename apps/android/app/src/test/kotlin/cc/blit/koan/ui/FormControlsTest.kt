package cc.blit.koan.ui

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.isSelected
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performImeAction
import androidx.compose.ui.test.performTextInput
import cc.blit.koan.theme.KoanField
import cc.blit.koan.theme.KoanFormRow
import cc.blit.koan.theme.KoanPicker
import cc.blit.koan.theme.KoanSegmented
import cc.blit.koan.theme.KoanStepper
import cc.blit.koan.theme.KoanTheme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The form controls the Settings and sign-in pages are made of, composed on the
 * JVM. This checks they compose, take input and report it; how they look and how
 * a real keyboard behaves needs a device.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = android.app.Application::class)
class FormControlsTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun `a segmented control selects, marks the choice and lowercases its words`() {
        val chosen = mutableListOf<String>()
        compose.setContent {
            var selected by remember { mutableStateOf("track") }
            KoanTheme {
                KoanSegmented(listOf("Off" to "off", "Track" to "track", "Album" to "album"), selected, { chosen += it; selected = it })
            }
        }
        compose.onNodeWithText("track").assert(isSelected())
        compose.onNodeWithText("album").performClick()
        assertEquals(listOf("album"), chosen)
        compose.onNodeWithText("album").assert(isSelected())
    }

    @Test
    fun `a disabled segmented control takes no tap`() {
        var taps = 0
        compose.setContent {
            KoanTheme { KoanSegmented(listOf("a" to 1, "b" to 2), 1, { taps++ }, enabled = false) }
        }
        compose.onNodeWithText("b").performClick()
        assertEquals(0, taps)
    }

    @Test
    fun `a field takes text and reports it, and its keyboard action commits`() {
        var text by mutableStateOf("")
        var done = 0
        compose.setContent {
            KoanTheme { KoanField(text, { text = it }, placeholder = "server url", onDone = { done++ }) }
        }
        compose.onNodeWithText("server url").assertExists()
        compose.onNode(hasSetTextAction()).performTextInput("https://x")
        assertEquals("https://x", text)
        compose.onNode(hasSetTextAction()).performImeAction()
        assertEquals(1, done)
    }

    @Test
    fun `a secret field hides its text until asked, and the placeholder goes once there is text`() {
        var text by mutableStateOf("")
        compose.setContent {
            KoanTheme { KoanField(text, { text = it }, placeholder = "password", secure = true) }
        }
        compose.onNodeWithText("show").assertDoesNotExist()
        compose.onNode(hasSetTextAction()).performTextInput("hunter2")
        compose.onNodeWithText("password").assertDoesNotExist()
        // Hidden: the semantic text is the mask, not the secret.
        val shown = compose.onNode(hasSetTextAction()).fetchSemanticsNode().config[SemanticsProperties.EditableText].text
        assertTrue("secret is masked", !shown.contains("hunter2"))
        compose.onNodeWithText("show").performClick()
        val revealed = compose.onNode(hasSetTextAction()).fetchSemanticsNode().config[SemanticsProperties.EditableText].text
        assertEquals("hunter2", revealed)
    }

    @Test
    fun `a stepper steps both ways and stops at its ends`() {
        var value by mutableStateOf(2)
        compose.setContent {
            KoanTheme {
                KoanStepper("workers", value.toString(), { value-- }, { value++ }, canDecrement = value > 1, canIncrement = value < 3)
            }
        }
        compose.onNodeWithContentDescription("Increase workers").performClick()
        assertEquals(3, value)
        compose.onNodeWithContentDescription("Increase workers").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Decrease workers").performClick()
        compose.onNodeWithContentDescription("Decrease workers").performClick()
        assertEquals(1, value)
        compose.onNodeWithContentDescription("Decrease workers").assertIsNotEnabled()
    }

    @Test
    fun `a picker shows the choice and picks another from its menu`() {
        var picked = "hour"
        compose.setContent {
            var selected by remember { mutableStateOf(picked) }
            KoanTheme {
                KoanPicker("every", listOf("15 minutes" to "quarter", "Hour" to "hour", "Day" to "day"), selected, { picked = it; selected = it })
            }
        }
        compose.onNodeWithText("hour").assertExists()
        compose.onNodeWithText("every").performClick()
        compose.onNodeWithText("day").performClick()
        assertEquals("day", picked)
    }

    @Test
    fun `a form row shows its label and value`() {
        compose.setContent { KoanTheme { KoanFormRow("Server", value = "Music.Example") } }
        compose.onNodeWithText("server").assertExists()
        compose.onNodeWithText("Music.Example").assertExists()
    }
}

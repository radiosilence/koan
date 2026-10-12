package cc.blit.koan.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onAllNodesWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import cc.blit.koan.nav.Tab
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanGlyph
import cc.blit.koan.theme.KoanIcon
import cc.blit.koan.theme.KoanTab
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTabBar
import cc.blit.koan.theme.KoanTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The theme's controls, composed on the JVM by Robolectric. This checks that
 * they compose and take taps; it does not check how they look, which only a
 * device or an emulator can. The app's own `Application` turns StrictMode to death in a
 * debug build, which Robolectric's own file reads would trip, so a plain one stands in.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = android.app.Application::class)
class ControlsTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun `the tab bar lowercases its labels, marks the selected tab, and tells select from reselect`() {
        val chosen = mutableListOf<Tab>()
        val again = mutableListOf<Tab>()
        compose.setContent {
            var selected by mutableStateOf(Tab.Queue)
            KoanTheme {
                KoanTabBar(Tab.entries.map { KoanTab(it, it.label) }, selected, { chosen += it; selected = it }, { again += it })
            }
        }
        compose.onNodeWithText("queue").assertExists()
        compose.onNode(hasText("queue")).performClick()
        assertEquals(listOf(Tab.Queue), again)
        compose.onNodeWithText("library").performClick()
        assertEquals(listOf(Tab.Library), chosen)
        assertEquals(listOf(Tab.Queue), again)
    }

    @Test
    fun `a disabled button takes no tap and an enabled one does`() {
        var taps = 0
        compose.setContent {
            KoanTheme {
                KoanButton("play", { taps++ }, kind = KoanButtonKind.Prominent)
                KoanButton("remove", { taps += 100 }, kind = KoanButtonKind.Destructive, enabled = false)
            }
        }
        compose.onNodeWithText("play").assertIsEnabled().performClick()
        compose.onNodeWithText("remove").performClick()
        assertEquals(1, taps)
    }

    @Test
    fun `library text keeps its own case and the app's words are lowercase`() {
        compose.setContent {
            KoanTheme {
                KoanText("Sign In", data = false)
                KoanText("Björk — Homogenic", data = true)
            }
        }
        compose.onNodeWithText("sign in").assertExists()
        compose.onNodeWithText("Björk — Homogenic").assertExists()
    }

    @Test
    fun `every glyph in the set composes and draws, filled or not, in both appearances`() {
        compose.setContent {
            for (dark in listOf(true, false)) {
                KoanTheme(dark = dark) {
                    Column {
                        for (name in KoanGlyph.names) KoanIcon(name, description = name)
                    }
                }
            }
        }
        assertEquals(2, compose.onAllNodesWithContentDescription("heart-filled").fetchSemanticsNodes().size)
        compose.waitForIdle()
    }
}

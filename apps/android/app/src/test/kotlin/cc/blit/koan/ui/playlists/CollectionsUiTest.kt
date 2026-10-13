package cc.blit.koan.ui.playlists

import android.app.Application
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTouchInput
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The collection pages' pieces that need no engine, drawn in the kōan theme (the default look). */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class CollectionsUiTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun aNameIsAskedForAndTrimmed() {
        var named: String? = null
        compose.setContent { NameDialog("New Playlist", "Create", onDismiss = {}) { named = it } }
        compose.onNodeWithText("create").assertIsNotEnabled()
        compose.onNode(hasSetTextAction()).performTextInput("  Late Night  ")
        compose.onNodeWithText("create").assertIsEnabled().performClick()
        assertEquals("Late Night", named)
    }

    @Test
    fun aRenameStartsFromTheCurrentName() {
        var named: String? = null
        compose.setContent { NameDialog("Rename Playlist", "Rename", "Evening", onDismiss = {}) { named = it } }
        compose.onNodeWithText("rename").performClick()
        assertEquals("Evening", named)
    }

    @Test
    fun confirmingActsAndCloses() {
        var confirmed = false
        var dismissed = false
        compose.setContent {
            ConfirmDialog("Delete Playlist?", "It cannot be undone.", "Delete", onDismiss = { dismissed = true }) { confirmed = true }
        }
        compose.onNodeWithText("delete").performClick()
        assertEquals(true to true, confirmed to dismissed)
    }

    @Test
    fun cancellingOnlyCloses() {
        var confirmed = false
        var dismissed = false
        compose.setContent {
            ConfirmDialog("Clear History?", "Every play is forgotten.", "Clear", onDismiss = { dismissed = true }) { confirmed = true }
        }
        compose.onNodeWithText("cancel").performClick()
        assertEquals(false to true, confirmed to dismissed)
    }

    @Test
    fun aRowPlaysOnTapAndOpensItsMenuOnHold() {
        var taps = 0
        var holds = 0
        compose.setContent { CollectionRow("Tone 260 Hz", "Tone Collective", onClick = { taps++ }, onLongClick = { holds++ }) }
        compose.onNodeWithText("Tone 260 Hz").performClick()
        compose.onNodeWithText("Tone 260 Hz").performTouchInput { longClick() }
        assertEquals(1 to 1, taps to holds)
    }

    @Test
    fun libraryTextKeepsItsCaseAndTheAppsWordsAreLowercased() {
        compose.setContent {
            CollectionRow("Evening Tones", null)
            MenuItem("Remove from Playlist") {}
        }
        compose.onNodeWithText("Evening Tones").assertExists()
        compose.onNodeWithText("remove from playlist").assertExists()
    }
}

package cc.blit.koan.ui.playlists

import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.unit.dp
import cc.blit.koan.art.ArtworkSource
import cc.blit.koan.theme.KoanTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The dialogs and the mosaic composed on the JVM by Robolectric: that they
 * compose in every shape and take input. Not how they look.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], application = android.app.Application::class)
class PlaylistDialogsTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun `the name dialog will not create until there is a name, and hands the typed name over`() {
        val named = mutableListOf<String>()
        compose.setContent {
            KoanTheme { NameDialog("new playlist", "Give it a name.", "", "create", {}, { named += it }) }
        }
        compose.onNodeWithText("create").assertIsNotEnabled()
        compose.onNode(hasSetTextAction()).performTextInput("  Road trip ")
        compose.onNodeWithText("create").assertIsEnabled().performClick()
        assertEquals(listOf("Road trip"), named)
    }

    @Test
    fun `clearing the name disables it again, and cancel dismisses`() {
        var dismissed = 0
        compose.setContent { KoanTheme { NameDialog("rename playlist", null, "Mix", "rename", { dismissed++ }, {}) } }
        compose.onNodeWithText("rename").assertIsEnabled()
        compose.onNode(hasSetTextAction()).performTextClearance()
        compose.onNodeWithText("rename").assertIsNotEnabled()
        compose.onNodeWithText("cancel").performClick()
        assertEquals(1, dismissed)
    }

    @Test
    fun `delete asks first, and the mosaic composes with none to four records`() {
        var deleted = 0
        compose.setContent {
            KoanTheme {
                (0..4).forEach { n -> PlaylistMosaic((1..n).map { ArtworkSource.Album(it.toLong()) }, 48.dp) }
                ConfirmDeleteDialog("Delete \"Mix\"?", {}, { deleted++ })
            }
        }
        compose.onNodeWithText("delete").performClick()
        assertEquals(1, deleted)
    }
}

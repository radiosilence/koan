package cc.blit.koan.ui.settings

import android.app.Application
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.dp
import cc.blit.koan.ui.theme.KoanTheme
import org.junit.Assert.assertFalse
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** A form row's label, measured with real fonts: one too long for its column wraps rather than losing its end. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(application = Application::class, sdk = [35])
class FormRowTextTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun aLongLabelWraps() {
        compose.setContent {
            KoanTheme {
                LazyColumn(Modifier.width(360.dp)) {
                    item { ChoiceRow("Nearby devices", "full", listOf("Full control" to "full")) {} }
                }
            }
        }
        val layouts = mutableListOf<TextLayoutResult>()
        compose.onNodeWithText("nearby devices").performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
        val label = layouts.single()
        assertFalse("${label.lineCount} lines, ${label.size}", label.hasVisualOverflow)
    }
}

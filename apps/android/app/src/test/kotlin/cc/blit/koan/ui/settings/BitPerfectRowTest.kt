package cc.blit.koan.ui.settings

import android.app.Application
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import cc.blit.koan.ui.theme.KoanTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The bit-perfect switch is offered only where Android can hand a USB DAC the stream as it is. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class)
class BitPerfectRowTest {
    @get:Rule
    val compose = createComposeRule()

    private var changed: Boolean? = null

    private fun show() = compose.setContent { KoanTheme { LazyColumn { bitPerfect(true) { changed = it } } } }

    @Test
    @Config(sdk = [34])
    fun androidFourteenOffersIt() {
        show()
        compose.onNodeWithText("bit-perfect over usb").performClick()
        assertEquals(false, changed)
    }

    @Test
    @Config(sdk = [33])
    fun androidThirteenDoesNot() {
        show()
        compose.onNodeWithText("bit-perfect over usb").assertDoesNotExist()
    }
}

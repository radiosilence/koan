package cc.blit.koan.ui

import android.app.Application
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import cc.blit.koan.ui.theme.KoanTheme
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** The app's shell over an engine that would not open, and while it is busy. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ShellTest {
    @get:Rule
    val compose = createComposeRule()

    @Test fun anEngineThatWouldNotOpenSaysWhy() {
        compose.setContent { KoanTheme { CouldNotStart("The library database is newer than this app") } }
        compose.onNodeWithText("could not start", substring = true).assertExists()
        compose.onNodeWithText("The library database is newer than this app", substring = true, ignoreCase = true).assertExists()
    }

    @Test fun theCardListsWhatIsRunning() {
        compose.setContent {
            KoanTheme { ActivityCard(listOf(ActivityTask(1, "Loading the library", unit = "tracks", done = 12_400, total = 49_700))) }
        }
        compose.onNodeWithText("Loading the library", ignoreCase = true).assertExists()
        compose.onNodeWithText("12,400 of 49,700 tracks", ignoreCase = true).assertExists()
    }
}

package cc.blit.koan.ui.navigation

import android.app.Application
import androidx.compose.foundation.clickable
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.navigation3.runtime.NavBackStack
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberSaveableStateHolderNavEntryDecorator
import androidx.navigation3.ui.NavDisplay
import kotlinx.serialization.Serializable
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@Serializable private data object Page : NavKey

/** Choosing the open tab again, as a tab bar does: back to its first page, then to its top. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class NavigatorTest {
    @get:Rule
    val compose = createComposeRule()

    private val stacks = Tab.all.associateWith { NavBackStack<NavKey>(it) }
    private val navigator = Navigator(stacks, mutableIntStateOf(Tab.all.indexOf(Tab.Settings)), NavBackStack(Shell))

    @Test fun theOpenTabReturnsToItsFirstPage() {
        navigator.push(Page)
        navigator.select(Tab.Settings)
        assertEquals(listOf<NavKey>(Tab.Settings), stacks.getValue(Tab.Settings).toList())
        assertEquals(0, navigator.reset(Tab.Settings))
    }

    @Test fun atItsFirstPageTheTabIsMadeAgain() {
        navigator.select(Tab.Settings)
        assertEquals(1, navigator.reset(Tab.Settings))
        assertEquals(0, navigator.reset(Tab.Library))
    }

    @Test fun anotherTabIsOpenedNotReset() {
        navigator.select(Tab.Library)
        assertEquals(Tab.Library, navigator.tab)
        assertEquals(0, navigator.reset(Tab.Library))
    }

    @Test fun aRemadeFirstPageKeepsNothingItSaved() {
        compose.setContent {
            NavDisplay(
                backStack = stacks.getValue(Tab.Settings),
                onBack = {},
                entryDecorators = listOf(rememberSaveableStateHolderNavEntryDecorator()),
                entryProvider = navigator.resetting(
                    entryProvider {
                        entry<Tab.Settings> {
                            var taps by rememberSaveable { mutableStateOf(0) }
                            Text("taps $taps", Modifier.clickable { taps++ })
                        }
                    },
                ),
            )
        }
        compose.onNodeWithText("taps 0").performClick()
        compose.onNodeWithText("taps 1").assertExists()
        compose.runOnIdle { navigator.select(Tab.Settings) }
        compose.onNodeWithText("taps 0").assertExists()
    }
}

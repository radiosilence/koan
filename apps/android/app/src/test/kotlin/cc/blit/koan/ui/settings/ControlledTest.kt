package cc.blit.koan.ui.settings

import android.app.Application
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import cc.blit.koan.engine.Devices
import cc.blit.koan.ffi.DeviceInfo
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.theme.KoanTheme
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** "Playing on <device>": what the transport says while this phone controls another, and the way back. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ControlledTest {
    @get:Rule
    val compose = createComposeRule()

    private fun mac(awake: Boolean = true, asleep: Boolean = false, wakeable: Boolean = false, waking: String? = null, problem: String? = null) =
        DeviceInfo(
            id = "mac", name = "Studio Mac", platform = "macos", account = true, owner = null, nearby = false,
            awake = awake, asleep = asleep, wakeable = wakeable, lastSeen = null, waking = waking, wakeFailed = null,
            sameLibrary = true, state = PlayState.PLAYING, title = null, artist = null, album = null,
            trackId = null, albumId = null, positionMs = 0u, durationMs = 0u, problem = problem,
        )

    @Test fun nothingWhileThisPhonePlays() {
        assertNull(controlled(Devices(listOf(mac()), target = null)))
    }

    @Test fun aReachableDeviceIsWhereTheMusicPlays() {
        val c = controlled(Devices(listOf(mac()), "mac"))!!
        assertEquals("Playing on Studio Mac", c.headline)
        assertNull(c.detail)
    }

    @Test fun anUnreachableDeviceSaysSoPlainly() {
        val c = controlled(Devices(listOf(mac(awake = false, asleep = true, problem = "Not on this network, and the server can't reach it.")), "mac"))!!
        assertEquals("Studio Mac is out of reach", c.headline)
        assertEquals("Not on this network, and the server can't reach it. Commands can't reach it.", c.detail)
        // Asleep with nothing to wake it: a Mac with kōan quit.
        assertEquals(Reach.OutOfReach(null), controlled(Devices(listOf(mac(awake = false, asleep = true)), "mac"))!!.reach)
        // Gone from the list entirely, as after a restart off the network.
        assertEquals("Another device is out of reach", controlled(Devices(emptyList(), "mac"))!!.headline)
    }

    @Test fun wakingAndAsleepAreNotOutOfReach() {
        assertEquals("Waking Studio Mac…", controlled(Devices(listOf(mac(awake = false, asleep = true, wakeable = true, waking = "push")), "mac"))!!.headline)
        val asleep = controlled(Devices(listOf(mac(awake = false, asleep = true, wakeable = true)), "mac"))!!
        assertEquals(Reach.Asleep, asleep.reach)
        assertEquals("A command wakes it.", asleep.detail)
        assertEquals("Reconnecting to Studio Mac…", controlled(Devices(listOf(mac(awake = false, asleep = false)), "mac"))!!.headline)
    }

    @Test fun oneTapGoesBackToThisPhone() {
        var back = 0
        var chose = 0
        compose.setContent {
            KoanTheme { ControlledLine(Controlled("Studio Mac", Reach.OutOfReach(null)), back = { back++ }, choose = { chose++ }) }
        }
        compose.onNodeWithText("studio mac is out of reach", ignoreCase = true).performClick()
        compose.onNodeWithText("use this phone").performClick()
        assertEquals(1 to 1, back to chose)
    }
}

package cc.blit.koan.ui.settings

import android.app.Application
import cc.blit.koan.engine.Devices
import cc.blit.koan.ffi.DeviceInfo
import cc.blit.koan.ffi.DeviceReach
import cc.blit.koan.ffi.PlayState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** "Playing on <device>": what the mini player says while this phone controls another. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class ControlledTest {
    private fun mac(waking: String? = null) =
        DeviceInfo(
            id = "mac", name = "Studio Mac", platform = "macos", account = true, owner = null, nearby = false,
            awake = waking == null, asleep = waking != null, wakeable = waking != null, lastSeen = null, waking = waking, wakeFailed = null,
            sameLibrary = true, state = PlayState.PLAYING, title = null, artist = null, album = null,
            trackId = null, albumId = null, positionMs = 0u, durationMs = 0u, problem = null,
        )

    private fun reaching(reach: DeviceReach?, vararg devices: DeviceInfo) = controlled(Devices(devices.toList(), "mac", reach))!!

    @Test fun nothingWhileThisPhonePlays() {
        assertNull(controlled(Devices(listOf(mac()), target = null)))
    }

    @Test fun aReachableDeviceIsWhereTheMusicPlays() {
        assertEquals("Playing on Studio Mac", reaching(DeviceReach.Connected, mac()).headline)
    }

    @Test fun anUnreachableDeviceSaysSoPlainly() {
        val c = reaching(DeviceReach.OutOfReach("Not on this network, and the server can't reach it."), mac())
        assertEquals("Studio Mac is out of reach", c.headline)
        assertEquals(Reach.OutOfReach("Not on this network, and the server can't reach it."), c.reach)
        // Gone from the list entirely, as after a restart off the network.
        assertEquals("Another device is out of reach", reaching(DeviceReach.OutOfReach(null)).headline)
    }

    @Test fun wakingAsleepAndReconnectingAreNotOutOfReach() {
        assertEquals("Waking Studio Mac…", reaching(DeviceReach.Waking, mac(waking = "push")).headline)
        assertEquals(Reach.Asleep, reaching(DeviceReach.Asleep, mac()).reach)
        assertEquals("Reconnecting to Studio Mac…", reaching(DeviceReach.Reconnecting, mac()).headline)
    }
}

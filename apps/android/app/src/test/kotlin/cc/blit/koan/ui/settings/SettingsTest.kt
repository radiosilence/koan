package cc.blit.koan.ui.settings

import android.app.Application
import android.content.Intent
import android.net.Uri
import cc.blit.koan.ffi.DeviceInfo
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ffi.SyncPhase
import cc.blit.koan.ffi.SyncProgress
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

/** What Settings works out without an engine: links it answers, devices it can move music to, and how it words things. */
@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class SettingsTest {
    private val context = RuntimeEnvironment.getApplication()

    private fun opens(link: String): Boolean {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(link))
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .setPackage(context.packageName)
        return context.packageManager.queryIntentActivities(intent, 0).any { it.activityInfo.name == "cc.blit.koan.MainActivity" }
    }

    @Test
    fun invitesAndPairingLinksOpenTheApp() {
        assertTrue(opens("https://koan.rocks/join/#server=https%3A%2F%2Fmusic.example.com&username=droid&invite=abc"))
        assertTrue(opens("https://koan.rocks/pair/#s=https%3A%2F%2Fmusic.example.com&p=abc"))
        assertTrue(opens("koan://join?server=https%3A%2F%2Fmusic.example.com&username=droid&invite=abc"))
    }

    @Test
    fun theRestOfTheSiteStaysInTheBrowser() {
        assertFalse(opens("https://koan.rocks/"))
        assertFalse(opens("https://koan.rocks/docs/"))
        assertFalse(opens("https://example.com/join/"))
    }

    private fun device(
        id: String = "tv",
        sameLibrary: Boolean = true,
        state: PlayState = PlayState.PLAYING,
        awake: Boolean = true,
        asleep: Boolean = false,
        wakeable: Boolean = false,
        lastSeen: Long? = null,
        problem: String? = null,
    ) = DeviceInfo(
        id = id, name = "Living room", platform = "tvos", account = true, owner = null, nearby = false,
        awake = awake, asleep = asleep, wakeable = wakeable, lastSeen = lastSeen, waking = null, wakeFailed = null,
        sameLibrary = sameLibrary, state = state, title = "Tone 300 Hz", artist = "Tone Settings", album = null,
        trackId = null, albumId = null, positionMs = 0u, durationMs = 0u, problem = problem,
    )

    @Test
    fun musicMovesOnlyWhereThereIsSomethingToMoveAndALibraryToPlayIt() {
        val tv = device()
        // From this phone: only with a queue, only to a device with the same library.
        assertTrue(canMoveMusic(tv, target = null, controlled = null, hasQueue = true))
        assertFalse(canMoveMusic(tv, target = null, controlled = null, hasQueue = false))
        assertFalse(canMoveMusic(device(sameLibrary = false), target = null, controlled = null, hasQueue = true))
        assertFalse(canMoveMusic(device(problem = "Out of reach"), target = null, controlled = null, hasQueue = true))
        // Back here from a device controlled: only while it has music.
        assertTrue(canMoveMusic(null, target = "tv", controlled = tv, hasQueue = false))
        assertFalse(canMoveMusic(null, target = "tv", controlled = device(state = PlayState.STOPPED), hasQueue = true))
        // Not to the device already controlled.
        assertFalse(canMoveMusic(tv, target = "tv", controlled = tv, hasQueue = true))
    }

    @Test
    fun aDeviceSaysWhatItIsDoing() {
        assertEquals("Playing Tone 300 Hz — Tone Settings", detail(device()))
        assertEquals("Idle · different library", detail(device(state = PlayState.STOPPED, sameLibrary = false)))
        assertEquals("Out of reach", detail(device(problem = "Out of reach")))
        assertEquals("Reconnecting…", detail(device(awake = false)))
        val now = 1_800_000_000_000L
        assertEquals(
            "Asleep · seen 5 minutes ago. Choosing it wakes it; music sent here arrives as a notification to tap.",
            detail(device(awake = false, asleep = true, wakeable = true, lastSeen = now / 1000 - 300), now),
        )
    }

    @Test
    fun syncProgressCountsWhatIsDone() {
        assertEquals("1,200 of 5,000 tracks", describe(SyncProgress(SyncPhase.TRACKS, 1200u, 5000u)))
        assertEquals("40 records", describe(SyncProgress(SyncPhase.ALBUMS, 40u, null)))
        assertEquals("Finishing", describe(SyncProgress(SyncPhase.FINISHING, 0u, null)))
    }

    @Test
    fun aSyncUnderAMinuteOldWasJustNow() {
        val now = 1_800_000_000_000L
        assertEquals("Just now", ago(now / 1000 - 20, now))
        assertEquals("3 minutes ago", ago(now / 1000 - 180, now))
    }

    @Test
    fun aPairingSaysWhereItCameFrom() {
        val local = PairingRequest("ABCD-EFGH", "Living room", "192.168.1.20", true, "droid", "music.example.com")
        assertEquals("Requested from 192.168.1.20, on your network.", local.origin)
        assertTrue(local.copy(local = false).origin.contains("from the internet"))
        assertEquals("music.example.com", host("https://music.example.com/"))
        assertEquals("10.0.2.2", host("http://10.0.2.2:4814"))
    }
}

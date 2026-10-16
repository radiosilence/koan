package cc.blit.koan.ui.settings

import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.Settings
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** AccountModel and SettingsModel against a stand-in engine. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = android.app.Application::class, sdk = [35])
class ModelsTest {
    private val dispatcher = StandardTestDispatcher()
    private val fake = FakeEngine()
    private var current = settings()

    @Before fun setUp() {
        Dispatchers.setMain(dispatcher)
        fake.answers["settings"] = { current }
        fake.answers["forgetRemote"] = { 3uL }
    }

    @After fun tearDown() = Dispatchers.resetMain()

    private fun account() = AccountModel({ fake.engine })

    private fun settingsModel() = SettingsModel({ fake.engine })

    @Test fun anInviteIsReadByTheEngine() = runTest(dispatcher) {
        val invite = invite()
        fake.answers["parseInvite"] = { (link) -> if (link == invite.link) invite else null }
        val model = account()
        assertEquals(invite, model.parseInvite(invite.link))
        assertNull(model.parseInvite("https://example.com"))
    }

    @Test fun anInviteForTheAccountSignedInJoinsWithoutAsking() = runTest(dispatcher) {
        val model = account()
        model.offer(invite(server = current.remoteUrl, username = current.remoteUsername))
        advanceUntilIdle()
        assertNull(model.state.value.pendingInvite)
        assertEquals(1, fake.called("joinInvite").size)
        assertEquals(true, model.state.value.signedIn)
        assertEquals(1, fake.called("syncRemote").size)
    }

    @Test fun anInviteForAnotherAccountAsksFirst() = runTest(dispatcher) {
        val model = account()
        val other = invite()
        model.offer(other)
        advanceUntilIdle()
        assertEquals(other, model.state.value.pendingInvite)
        assertTrue(fake.called("joinInvite").isEmpty())

        model.join(other)
        advanceUntilIdle()
        assertNull(model.state.value.pendingInvite)
        assertEquals(listOf(listOf<Any?>(other)), fake.called("joinInvite"))
    }

    @Test fun anInviteWithNothingSignedInJoins() = runTest(dispatcher) {
        current = settings(signedIn = false)
        val model = account()
        model.offer(invite())
        advanceUntilIdle()
        assertNull(model.state.value.pendingInvite)
        assertEquals(1, fake.called("joinInvite").size)
    }

    @Test fun aRefusedSignInSaysWhyAndStaysSignedOut() = runTest(dispatcher) {
        current = settings(signedIn = false)
        fake.answers["signInRemote"] = { throw KoanException.Remote("wrong password") }
        val model = account()
        advanceUntilIdle()
        model.signIn(" https://music.example.com ", "droid", "nope", withApiKey = false)
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>("https://music.example.com", "droid", "nope")), fake.called("signInRemote"))
        assertEquals("wrong password", model.state.value.error)
        assertEquals(false, model.state.value.signedIn)
        assertNull(model.state.value.busy)
    }

    @Test fun signingOutForgetsTheServerAndReturnsToSignIn() = runTest(dispatcher) {
        val account = account()
        val model = settingsModel()
        advanceUntilIdle()
        assertEquals(true, account.state.value.signedIn)
        model.signOut(forgetTracks = true, then = account::signedOut)
        advanceUntilIdle()
        assertEquals(1, fake.called("signOutRemote").size)
        assertEquals(1, fake.called("forgetRemote").size)
        assertEquals(false, account.state.value.signedIn)
    }

    @Test fun signingOutKeepingTracksForgetsNothing() = runTest(dispatcher) {
        val model = settingsModel()
        model.signOut(forgetTracks = false) {}
        advanceUntilIdle()
        assertEquals(1, fake.called("signOutRemote").size)
        assertTrue(fake.called("forgetRemote").isEmpty())
    }

    @Test fun aFailedSignOutStaysSignedIn() = runTest(dispatcher) {
        fake.answers["signOutRemote"] = { throw KoanException.Database("locked") }
        var out = false
        val model = settingsModel()
        model.signOut(forgetTracks = true) { out = true }
        advanceUntilIdle()
        assertFalse(out)
        assertEquals("locked", model.state.value.error)
    }

    @Test fun anEditIsShownAtOnceAndWrittenWhole() = runTest(dispatcher) {
        val model = settingsModel()
        advanceUntilIdle()
        model.edit { it.copy(replaygain = "album") }
        assertEquals("album", model.state.value.settings?.replaygain)
        advanceUntilIdle()
        val written = fake.called("updateSettings").single().single() as Settings
        assertEquals(current.copy(replaygain = "album"), written)
    }

    @Test fun aRefusedEditSaysWhy() = runTest(dispatcher) {
        fake.answers["updateSettings"] = { throw KoanException.BadArgument("not a size: 5 parsecs") }
        val model = settingsModel()
        advanceUntilIdle()
        model.edit { it.copy(cacheLimit = "5 parsecs") }
        advanceUntilIdle()
        assertEquals("not a size: 5 parsecs", model.state.value.error)
    }

    @Test fun reloadingPicksUpAChangeMadeElsewhere() = runTest(dispatcher) {
        val model = settingsModel()
        advanceUntilIdle()
        assertEquals("off", model.state.value.settings?.replaygain)
        current = current.copy(replaygain = "track")
        model.reload()
        advanceUntilIdle()
        assertEquals("track", model.state.value.settings?.replaygain)
    }
}

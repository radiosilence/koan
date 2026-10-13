package cc.blit.koan.state

import cc.blit.koan.FakeEngine
import cc.blit.koan.record
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.koan_ffi.KoanException
import uniffi.koan_ffi.PairingCode
import uniffi.koan_ffi.PairingInfo
import uniffi.koan_ffi.Settings

@OptIn(ExperimentalCoroutinesApi::class)
class SettingsModelTest {
    private val mirror = EngineMirror()

    /** An engine holding [config], recording what is written to it. */
    private class Held(var config: Settings) {
        val writes = mutableListOf<Settings>()
        var invite: uniffi.koan_ffi.Invite? = null
        var fail: String? = null
    }

    private fun held(config: Settings = settingsRecord()) = Held(config)

    private fun engineOf(h: Held, extra: (String, List<Any?>) -> Any? = { _, _ -> null }) = FakeEngine { name, args ->
        when (name) {
            "settings" -> h.config
            "appearance" -> appearanceRecord()
            "updateSettings" -> {
                h.writes += args[0] as Settings
                h.fail?.let { throw KoanException.BadArgument(it) }
                h.config = args[0] as Settings
                Unit
            }
            "parseInvite" -> h.invite
            else -> extra(name, args)
        }
    }

    private fun TestScope.model(engine: FakeEngine) = SettingsModel(engine.engine, mirror, backgroundScope)

    private fun FakeEngine.called(prefix: String) = synchronized(calls) { calls.any { it.startsWith(prefix) } }

    @Test
    fun `no settings yet is loading`() {
        assertEquals(ServerStatus.Loading, SettingsModel.status(null, refused = false, linked = false))
    }

    @Test
    fun `the status follows the configuration, a refusal and the link`() {
        assertEquals(ServerStatus.SignedOut, SettingsModel.status(settingsRecord(signedIn = false), false, false))
        assertEquals(ServerStatus.Refused("https://music.example"), SettingsModel.status(settingsRecord(), true, true))
        assertEquals(
            ServerStatus.SignedIn("https://music.example", "ada", linked = true),
            SettingsModel.status(settingsRecord(), false, true),
        )
    }

    @Test
    fun `refresh reads the configuration and the appearance`() = runTest {
        val h = held()
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        assertEquals(h.config, model.settings.value)
        assertNotNull(model.appearance.value)
    }

    @Test
    fun `re-reading on focus picks up a change made elsewhere`() = runTest {
        val h = held()
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        assertTrue(model.settings.value!!.autoSync)
        h.config = h.config.copy(autoSync = false)
        model.refresh(); runCurrent()
        assertFalse(model.settings.value!!.autoSync)
    }

    @Test
    fun `an edit shows at once and is written whole`() = runTest {
        val h = held()
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        model.setFadeOnPause(true)
        assertTrue("the state moves before the write", model.settings.value!!.fadeOnPause)
        runCurrent()
        assertEquals(1, h.writes.size)
        assertTrue(h.writes.single().fadeOnPause)
        assertEquals("the rest of the configuration is written as it was", h.config.autoSync, h.writes.single().autoSync)
    }

    @Test
    fun `an edit before the configuration is read writes nothing`() = runTest {
        val h = held()
        val model = model(engineOf(h))
        model.setFadeOnPause(true); runCurrent()
        assertTrue(h.writes.isEmpty())
    }

    @Test
    fun `quick edits end with the last one written`() = runTest {
        val h = held()
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        model.setFadeOnPause(true)
        model.setReplayGain("album")
        model.setPreAmp(2.5)
        runCurrent()
        val last = h.writes.last()
        assertTrue(last.fadeOnPause)
        assertEquals("album", last.replaygain)
        assertEquals(2.5, last.preAmpDb, 0.0)
    }

    @Test
    fun `a failed write shows its message and the next read shows what is held`() = runTest {
        val h = held().also { it.fail = "disk full" }
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        model.setAutoSync(false); runCurrent()
        assertEquals("disk full", model.error.value)
        model.refresh(); runCurrent()
        assertTrue(model.settings.value!!.autoSync)
    }

    @Test
    fun `a cache limit is normalised, blank clears it, and a bad one writes nothing`() = runTest {
        val h = held()
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        assertTrue(model.setCacheLimit("50gb")); runCurrent()
        assertEquals("50GB", h.config.cacheLimit)
        assertTrue(model.setCacheLimit("")); runCurrent()
        assertEquals("", h.config.cacheLimit)
        val writes = h.writes.size
        assertFalse(model.setCacheLimit("lots")); runCurrent()
        assertEquals("a refused limit is not written", writes, h.writes.size)
        assertTrue(model.cacheLimitRejected.value)
        assertNotNull(model.error.value)
        assertTrue(model.setCacheLimit("")); runCurrent()
        assertFalse(model.cacheLimitRejected.value)
    }

    @Test
    fun `committing a limit that is already held writes nothing`() = runTest {
        val h = held(settingsRecord().copy(cacheLimit = "50GB"))
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        model.setCacheLimit("50gb"); runCurrent()
        assertTrue(h.writes.isEmpty())
    }

    @Test
    fun `workers and pre-amp are held to the engine's range`() = runTest {
        val h = held()
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        model.setDownloadWorkers(40); runCurrent()
        assertEquals(16u, h.config.downloadWorkers)
        model.setDownloadWorkers(-3); runCurrent()
        assertEquals(1u, h.config.downloadWorkers)
        model.setPreAmp(-99.0); runCurrent()
        assertEquals(-15.0, h.config.preAmpDb, 0.0)
    }

    @Test
    fun `an address is validated, added once, and removable`() = runTest {
        val h = held()
        val model = model(engineOf(h))
        model.refresh(); runCurrent()
        assertFalse(model.addAddress("http://nas")); runCurrent()
        assertTrue(h.writes.isEmpty())
        assertNotNull(model.error.value)
        assertTrue(model.addAddress(" nas.local:5626 ")); runCurrent()
        assertTrue(model.addAddress("nas.local:5626")); runCurrent()
        assertEquals(listOf("nas.local:5626"), h.config.devicesAddresses)
        model.removeAddress("nas.local:5626"); runCurrent()
        assertTrue(h.config.devicesAddresses.isEmpty())
    }

    @Test
    fun `ending a connection with refuse remembers its address and hangs up`() = runTest {
        val h = held()
        val engine = engineOf(h)
        val model = model(engine)
        model.refresh(); runCurrent()
        val c = uniffi.koan_ffi.ConnectedInfo(
            key = 7uL, viaServer = false, inbound = true, name = "stranger", addr = "10.0.0.9", own = false,
            owner = null, since = null,
        )
        model.endConnection(c, refuse = true); runCurrent()
        assertEquals(listOf("10.0.0.9"), h.config.devicesRefused)
        assertTrue(engine.called("endConnection(7"))
        model.allowRefused("10.0.0.9"); runCurrent()
        assertTrue(h.config.devicesRefused.isEmpty())
    }

    @Test
    fun `record colours is saved through the engine and shown at once`() = runTest {
        val h = held()
        val engine = engineOf(h)
        val model = model(engine)
        model.refresh(); runCurrent()
        model.setRecordColours(false)
        assertFalse(model.appearance.value!!.recordColours)
        runCurrent()
        assertTrue(engine.called("setRecordColours(false"))
    }

    // -- Signing in ---------------------------------------------------------------------------------

    @Test
    fun `signing in needs all three fields and asks the engine nothing without them`() = runTest {
        val engine = engineOf(held())
        val model = model(engine)
        model.signIn("https://x", "ada", ""); runCurrent()
        assertNotNull(model.error.value)
        assertFalse(engine.called("signInRemote"))
        model.signIn("  ", "ada", "pw"); runCurrent()
        assertFalse(engine.called("signInRemote"))
    }

    @Test
    fun `a password signs in with trimmed address and name, then reads the configuration and syncs`() = runTest {
        val engine = engineOf(held())
        val model = model(engine)
        model.signIn(" https://x ", " ada ", "pw"); runCurrent()
        assertTrue(engine.called("signInRemote(https://x, ada, pw)"))
        assertFalse(model.signingIn.value)
        assertNull(model.error.value)
        assertNotNull(model.settings.value)
        assertTrue(engine.called("syncRemote"))
    }

    @Test
    fun `an api key signs in through the key call`() = runTest {
        val engine = engineOf(held())
        val model = model(engine)
        model.signIn("https://x", "ada", "key", withApiKey = true); runCurrent()
        assertTrue(engine.called("signInRemoteWithKey("))
        assertFalse(engine.called("signInRemote(https"))
    }

    @Test
    fun `a refused sign-in shows the server's reason, never the secret, and does not sync`() = runTest {
        val engine = engineOf(held()) { name, _ ->
            if (name == "signInRemote") throw KoanException.BadArgument("401 unauthorised") else null
        }
        val model = model(engine)
        model.signIn("https://x", "ada", "hunter2"); runCurrent()
        assertEquals("401 unauthorised", model.error.value)
        assertFalse(model.error.value!!.contains("hunter2"))
        assertFalse(model.signingIn.value)
        assertFalse(engine.called("syncRemote"))
    }

    @Test
    fun `a second sign-in while one runs is ignored`() = runTest {
        val engine = engineOf(held())
        val model = model(engine)
        model.signIn("https://x", "ada", "pw")
        model.signIn("https://x", "ada", "pw"); runCurrent()
        assertEquals(1, synchronized(engine.calls) { engine.calls.count { it.startsWith("signInRemote(") } })
    }

    @Test
    fun `signing out can forget the server's tracks`() = runTest {
        val engine = engineOf(held()) { name, _ -> if (name == "forgetRemote") 3uL else null }
        val model = model(engine)
        model.signOut(forgetTracks = true); runCurrent()
        assertTrue(engine.called("signOutRemote"))
        assertTrue(engine.called("forgetRemote"))
        assertEquals("Signed out and forgot 3 tracks", model.result.value)
        val keep = engineOf(held())
        model(keep).signOut(forgetTracks = false); runCurrent()
        assertFalse(keep.called("forgetRemote"))
    }

    // -- Invites ------------------------------------------------------------------------------------

    @Test
    fun `text that is not an invite says so, unless the field is only probing`() = runTest {
        val h = held(settingsRecord(signedIn = false))
        val model = model(engineOf(h))
        model.offerInvite("https://music.example", quiet = true); runCurrent()
        assertNull(model.error.value)
        model.offerInvite("hello"); runCurrent()
        assertEquals("That isn't a kōan invite.", model.error.value)
    }

    @Test
    fun `an invite signed out joins at once and loads the library`() = runTest {
        val h = held(settingsRecord(signedIn = false)).also { it.invite = inviteRecord() }
        val engine = engineOf(h)
        val model = model(engine)
        model.offerInvite("https://koan.rocks/join/#x"); runCurrent()
        assertTrue(engine.called("joinInvite"))
        assertTrue(engine.called("syncRemote"))
        assertNull(model.pendingInvite.value)
    }

    @Test
    fun `an invite to the account already signed in joins without asking`() = runTest {
        val h = held().also { it.invite = inviteRecord() }
        val engine = engineOf(h)
        val model = model(engine)
        model.offerInvite("https://koan.rocks/join/#x"); runCurrent()
        assertTrue(engine.called("joinInvite"))
        assertNull(model.pendingInvite.value)
    }

    @Test
    fun `an invite to a different account waits for a yes`() = runTest {
        val h = held().also { it.invite = inviteRecord(server = "https://other.example", user = "bob") }
        val engine = engineOf(h)
        val model = model(engine)
        model.offerInvite("https://koan.rocks/join/#x"); runCurrent()
        assertEquals("bob", model.pendingInvite.value?.username)
        assertFalse(engine.called("joinInvite"))
        model.dismissInvite(); runCurrent()
        assertFalse(engine.called("joinInvite"))
        model.offerInvite("https://koan.rocks/join/#x"); runCurrent()
        model.confirmInvite(); runCurrent()
        assertTrue(engine.called("joinInvite"))
        assertNull(model.pendingInvite.value)
    }

    @Test
    fun `an invite that fails names the host and not the token`() = runTest {
        val h = held(settingsRecord(signedIn = false)).also { it.invite = inviteRecord() }
        val engine = engineOf(h) { name, _ -> if (name == "joinInvite") throw KoanException.BadArgument("expired") else null }
        val model = model(engine)
        model.offerInvite("x"); runCurrent()
        assertEquals("Could not sign in to music.example: expired", model.error.value)
        assertFalse(model.error.value!!.contains("tok"))
        assertFalse(engine.called("syncRemote"))
    }

    // -- Pairing ------------------------------------------------------------------------------------

    @Test
    fun `a typed code is normalised, asked of the server and put to the person`() = runTest {
        val h = held()
        val engine = engineOf(h) { name, _ -> if (name == "pairingInfo") PairingInfo("Living room TV", "192.168.1.5", true) else null }
        val model = model(engine)
        model.offerPairing(" abcd ef12 "); runCurrent()
        assertTrue(engine.called("pairingInfo(ABCD-EF12)"))
        val request = model.pendingPairing.value!!
        assertEquals("Living room TV", request.device)
        assertEquals("ada", request.username)
        assertEquals("music.example", request.host)
        assertTrue(request.origin.contains("on your network"))
    }

    @Test
    fun `allowing signs the device in and says so, declining says nothing more`() = runTest {
        val h = held()
        val engine = engineOf(h) { name, _ ->
            when (name) {
                "pairingInfo" -> PairingInfo("TV", "1.2.3.4", false)
                "approvePairing" -> "TV"
                else -> null
            }
        }
        val model = model(engine)
        model.offerPairing("ABCD-EF12"); runCurrent()
        assertFalse(model.pendingPairing.value!!.local)
        model.settlePairing(true); runCurrent()
        assertTrue(engine.called("approvePairing(ABCD-EF12)"))
        assertEquals("TV is signed in as ada.", model.result.value)
        assertNull(model.pendingPairing.value)
        model.offerPairing("ABCD-EF12"); runCurrent()
        model.settlePairing(false); runCurrent()
        assertTrue(engine.called("declinePairing(ABCD-EF12)"))
    }

    @Test
    fun `a code no device is waiting on is reported`() = runTest {
        val engine = engineOf(held()) { name, _ -> if (name == "pairingInfo") throw KoanException.NotFound("no such pairing") else null }
        val model = model(engine)
        model.offerPairing("ABCD-EF12"); runCurrent()
        assertNull(model.pendingPairing.value)
        assertTrue(model.error.value!!.startsWith("No device is waiting with that code"))
        model.offerPairing("   "); runCurrent()
        assertEquals("Enter the code the other device shows.", model.error.value)
    }

    @Test
    fun `a pairing link for another server is not approved from here`() = runTest {
        val engine = engineOf(held()) { name, _ ->
            if (name == "parsePairingLink") uniffi.koan_ffi.PairingLink("https://elsewhere.example", "id1") else null
        }
        val model = model(engine)
        model.offerPairingLink("https://koan.rocks/pair/#s=x&p=y"); runCurrent()
        assertNull(model.pendingPairing.value)
        assertTrue(model.error.value!!.contains("elsewhere.example"))
        assertFalse(engine.called("pairingInfo"))
    }

    @Test
    fun `a pairing link for this server is asked about by its id`() = runTest {
        val engine = engineOf(held()) { name, _ ->
            when (name) {
                "parsePairingLink" -> uniffi.koan_ffi.PairingLink("https://music.example/", "id1")
                "pairingInfo" -> PairingInfo("TV", "10.0.0.2", true)
                else -> null
            }
        }
        val model = model(engine)
        model.offerPairingLink("link"); runCurrent()
        assertTrue(engine.called("pairingInfo(id1)"))
        assertNotNull(model.pendingPairing.value)
    }

    @Test
    fun `this device shows a code, waits, and is signed in when it is approved`() = runTest {
        val engine = engineOf(held()) { name, _ ->
            if (name == "startPairing") PairingCode("id", "WXYZ-1234", "https://koan.rocks/pair/#p") else null
        }
        val model = model(engine)
        model.startPairing(" https://music.example "); runCurrent()
        assertTrue(engine.called("startPairing(https://music.example)"))
        assertTrue(engine.called("awaitPairing"))
        assertEquals(PairingState.Idle, model.pairing.value)
        assertNotNull("approval reads the configuration so the shell's gate can lift", model.settings.value)
    }

    @Test
    fun `the code is on offer between asking and approval, and giving up clears it`() = runTest {
        val engine = engineOf(held()) { name, _ ->
            if (name == "startPairing") PairingCode("id", "WXYZ-1234", "https://koan.rocks/pair/#p") else null
        }
        val model = model(engine)
        val seen = model.pairing.record(backgroundScope)
        model.startPairing("https://music.example"); runCurrent()
        assertEquals(
            listOf(PairingState.Idle, PairingState.Starting, PairingState.Waiting("WXYZ-1234", "https://koan.rocks/pair/#p"), PairingState.Idle),
            seen.toList(),
        )
        model.cancelPairing(); runCurrent()
        assertEquals(PairingState.Idle, model.pairing.value)
        assertTrue(engine.called("cancelPairing"))
    }

    @Test
    fun `a pairing that fails says why`() = runTest {
        val engine = engineOf(held()) { name, _ -> if (name == "startPairing") throw KoanException.BadArgument("server does not pair") else null }
        val model = model(engine)
        model.startPairing("https://music.example"); runCurrent()
        assertEquals(PairingState.Failed("server does not pair"), model.pairing.value)
    }

    // -- Messages -----------------------------------------------------------------------------------

    @Test
    fun `a result clears itself, an error stays`() = runTest {
        val engine = engineOf(held()) { name, _ ->
            if (name == "clearDownloadCache") uniffi.koan_ffi.CacheCleared(files = 3u, bytes = 1_048_576u) else null
        }
        val model = model(engine)
        model.clearCache(); runCurrent()
        assertEquals("Freed 1.0 MB across 3 files", model.result.value)
        advanceTimeBy(6_001); runCurrent()
        assertNull(model.result.value)
        model.report("boom")
        advanceTimeBy(60_000); runCurrent()
        assertEquals("boom", model.error.value)
    }

    @Test
    fun `slow work shows as a row of the activity registry for as long as it runs`() = runTest {
        val activity = ActivityModel()
        val seen = mutableListOf<List<String>>()
        val engine = engineOf(held()) { name, _ ->
            if (name == "syncRemote") seen += activity.tasks.value.map { it.label }
            if (name == "syncRemote") uniffi.koan_ffi.SyncSummary(0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u) else null
        }
        val model = model(engine)
        model.syncNow(activity); runCurrent()
        assertEquals(listOf(listOf("Syncing with server")), seen)
        assertTrue(activity.tasks.value.isEmpty())
        assertEquals("Already up to date", model.result.value)
    }
}

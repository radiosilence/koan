package cc.blit.koan.ui.settings

import cc.blit.koan.ffi.AccountRole
import cc.blit.koan.ffi.ApiKeyInfo
import cc.blit.koan.ffi.AppPasswordInfo
import cc.blit.koan.ffi.ConnectionInfo
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.NewApiKey
import cc.blit.koan.ffi.NewAppPassword
import cc.blit.koan.ffi.ServerAccount
import cc.blit.koan.ffi.ServerExtension
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

/** API keys, app passwords and the server's accounts, against a stand-in engine. */
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(application = android.app.Application::class, sdk = [35])
class AccountModelsTest {
    private val dispatcher = StandardTestDispatcher()
    private val fake = FakeEngine()
    private val keys = mutableListOf(ApiKeyInfo(1, "This phone", 1_700_000_000, null, true), ApiKeyInfo(2, "Feishin", null, null, false))

    @Before fun setUp() {
        Dispatchers.setMain(dispatcher)
        fake.answers["apiKeys"] = { keys.toList() }
        fake.answers["createApiKey"] = { (name) ->
            keys += ApiKeyInfo(3, name as String, null, null, false)
            NewApiKey(name, "secret-key")
        }
        fake.answers["appPasswords"] = { listOf(AppPasswordInfo(7, "Old player", null, null)) }
        fake.answers["createAppPassword"] = { (name) -> NewAppPassword(name as String, "app-secret") }
    }

    @After fun tearDown() = Dispatchers.resetMain()

    @Test fun keysAreListedAndThisDeviceIsMarked() = runTest(dispatcher) {
        val model = CredentialsModel.apiKeys { fake.engine }
        advanceUntilIdle()
        val items = model.state.value.items!!
        assertEquals(listOf("This phone", "Feishin"), items.map { it.name })
        assertTrue(items.first().thisDevice)
    }

    @Test fun aNewKeyIsShownOnceAndListed() = runTest(dispatcher) {
        val model = CredentialsModel.apiKeys { fake.engine }
        var made: Boolean? = null
        model.create("  Symfonium ") { made = it }
        advanceUntilIdle()
        assertEquals(true, made)
        assertEquals(listOf(listOf<Any?>("Symfonium")), fake.called("createApiKey"))
        assertEquals(MadeSecret("Symfonium", "secret-key"), model.state.value.made)
        assertEquals("Symfonium", model.state.value.items!!.last().name)
        model.dismissMade()
        assertNull(model.state.value.made)
    }

    @Test fun aBlankNameMakesNothing() = runTest(dispatcher) {
        val model = CredentialsModel.apiKeys { fake.engine }
        model.create("   ")
        advanceUntilIdle()
        assertTrue(fake.called("createApiKey").isEmpty())
    }

    @Test fun revokingAKeyListsAgain() = runTest(dispatcher) {
        fake.answers["revokeApiKey"] = { (id) -> keys.removeAll { it.id == id } }
        val model = CredentialsModel.apiKeys { fake.engine }
        advanceUntilIdle()
        model.revoke(model.state.value.items!!.last())
        advanceUntilIdle()
        assertEquals(listOf(listOf<Any?>(2L)), fake.called("revokeApiKey"))
        assertEquals(listOf("This phone"), model.state.value.items!!.map { it.name })
    }

    @Test fun aRefusalIsSaid() = runTest(dispatcher) {
        fake.answers["createAppPassword"] = { throw KoanException.Remote("name taken") }
        val model = CredentialsModel.appPasswords { fake.engine }
        var made: Boolean? = null
        model.create("Old player") { made = it }
        advanceUntilIdle()
        assertEquals(false, made)
        assertEquals("name taken", model.state.value.error)
        assertNull(model.state.value.made)
    }

    @Test fun anAccountThatIsNotAnAdminsSeesNoPeople() = runTest(dispatcher) {
        fake.answers["serverAccounts"] = { throw KoanException.Remote("forbidden") }
        val model = PeopleModel { fake.engine }
        advanceUntilIdle()
        assertNull(model.state.value.accounts)
    }

    @Test fun addingSomeoneMakesTheirInvite() = runTest(dispatcher) {
        val accounts = mutableListOf(ServerAccount("droid", AccountRole.ADMIN))
        fake.answers["serverAccounts"] = { accounts.toList() }
        fake.answers["createServerAccount"] = { (name, role) ->
            accounts += ServerAccount(name as String, role as AccountRole)
            invite(username = name)
        }
        val model = PeopleModel { fake.engine }
        var added: Boolean? = null
        model.create(" guest ", AccountRole.READONLY) { added = it }
        advanceUntilIdle()
        assertEquals(true, added)
        assertEquals("guest", model.state.value.invite?.username)
        assertEquals(listOf("droid", "guest"), model.state.value.accounts!!.map { it.username })
    }

    @Test fun aRefusedChangeListsWhatIsStored() = runTest(dispatcher) {
        fake.answers["serverAccounts"] = { listOf(ServerAccount("droid", AccountRole.ADMIN)) }
        fake.answers["setServerAccountRole"] = { throw KoanException.BadArgument("the last admin stays an admin") }
        val model = PeopleModel { fake.engine }
        model.setRole("droid", AccountRole.USER)
        advanceUntilIdle()
        assertEquals("the last admin stays an admin", model.state.value.error)
        assertEquals(AccountRole.ADMIN, model.state.value.accounts!!.single().role)
    }

    @Test fun theAccountPageIsThereOnlyWithSomethingOnIt() {
        fun offering(vararg names: String, offline: Boolean = false) = connection(names.toList(), offline)
        assertFalse(accountShown(null))
        assertFalse(accountShown(offering("koanLink")))
        assertTrue(accountShown(offering(PASSWORDS)))
        assertTrue(accountShown(offering(API_KEYS)))
        assertTrue(accountShown(offering(APP_PASSWORDS)))
        assertFalse(accountShown(offering(APP_PASSWORDS, offline = true)))
    }

    @Test fun integrationsAreThereOnlyWhereTheServerOffersOne() {
        // A koan server lists assistants only once it knows its public address.
        assertFalse(integrationsShown(connection(listOf("koanLink", "koanApiKeys"), offline = false)))
        assertTrue(integrationsShown(connection(listOf(ASSISTANTS), offline = false)))
        assertTrue(integrationsShown(connection(emptyList(), offline = false).copy(scrobbling = true)))
        assertFalse(integrationsShown(null))
    }

    @Test fun aCredentialSaysWhenItWasUsed() {
        assertEquals("never used", used(Credential(1, "x", null, null)))
        assertTrue(used(Credential(1, "x", 1_700_000_000, 1_760_000_000)).matches(Regex("Made .+ · last used .+")))
    }

    private fun connection(extensions: List<String>, offline: Boolean) = ConnectionInfo(
        serverKind = "koan", serverVersion = "0.61.13", openSubsonic = true,
        extensions = extensions.map { ServerExtension(it, listOf(1)) },
        devices = true, linked = true, listeningPort = null, localNetworkBlocked = false, thisDevice = "phone",
        sharing = false, sharedWith = emptyList(), shareError = null, shareAccounts = emptyList(),
        offline = offline, offlineManual = false, pairing = true, scrobbling = false, signInRefused = false,
        nearbyServers = emptyList(), commandNotice = null,
        connections = emptyList(), held = emptyList(),
    )
}

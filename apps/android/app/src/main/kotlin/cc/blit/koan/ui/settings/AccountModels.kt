package cc.blit.koan.ui.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.describe
import cc.blit.koan.ffi.AccountRole
import cc.blit.koan.ffi.Invite
import cc.blit.koan.ffi.KoanEngineInterface
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.ServerAccount
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** An API key or app password of the signed-in account, as a row shows it. */
data class Credential(val id: Long, val name: String, val created: Long?, val lastUsed: Long?, val thisDevice: Boolean = false)

/** A key or app password just made: shown this once, since the server keeps only its hash or a sealed copy. */
data class MadeSecret(val name: String, val secret: String)

data class CredentialsState(
    /** `null` until the server has answered. */
    val items: List<Credential>? = null,
    val made: MadeSecret? = null,
    val error: String? = null,
)

/**
 * The signed-in account's API keys or app passwords: listed, made and
 * revoked, the counterpart of `ApiKeysModel` and `AppPasswordsModel`.
 * Revoking a key is how a lost phone is cut off.
 */
class CredentialsModel(
    private val list: suspend () -> List<Credential>,
    private val make: suspend (String) -> MadeSecret,
    private val revokeOne: suspend (Long) -> Unit,
) : ViewModel() {
    private val _state = MutableStateFlow(CredentialsState())
    val state: StateFlow<CredentialsState> = _state.asStateFlow()

    init {
        viewModelScope.launch { load() }
    }

    /** Make one named `name`; whether it was made. */
    fun create(name: String, then: (Boolean) -> Unit = {}) {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return
        viewModelScope.launch {
            val made = attempt { make(trimmed) }
            if (made != null) _state.update { it.copy(made = made, error = null) }
            load()
            then(made != null)
        }
    }

    fun revoke(credential: Credential) {
        viewModelScope.launch {
            if (attempt { revokeOne(credential.id) } != null) _state.update { it.copy(error = null) }
            load()
        }
    }

    fun dismissMade() = _state.update { it.copy(made = null) }

    private suspend fun load() {
        attempt { list() }?.let { items -> _state.update { it.copy(items = items) } }
    }

    private suspend fun <T> attempt(work: suspend () -> T): T? = try {
        work()
    } catch (e: KoanException) {
        _state.update { it.copy(error = e.describe()) }
        null
    }

    companion object {
        fun apiKeys(engine: suspend () -> KoanEngineInterface) = CredentialsModel(
            list = { engine().apiKeys().map { Credential(it.id, it.name, it.created, it.lastUsed, it.thisDevice) } },
            make = { name -> engine().createApiKey(name).let { MadeSecret(it.name, it.key) } },
            revokeOne = { engine().revokeApiKey(it) },
        )

        fun appPasswords(engine: suspend () -> KoanEngineInterface) = CredentialsModel(
            list = { engine().appPasswords().map { Credential(it.id, it.name, it.created, it.lastUsed) } },
            make = { name -> engine().createAppPassword(name).let { MadeSecret(it.name, it.password) } },
            revokeOne = { engine().revokeAppPassword(it) },
        )
    }
}

data class PeopleState(
    /** `null` until listed, and for good where the server lists none: not koan, or not an admin. */
    val accounts: List<ServerAccount>? = null,
    /** An invite just made, ready to send. */
    val invite: Invite? = null,
    val error: String? = null,
)

/**
 * The signed-in server's accounts, for its admins: add someone, set what they
 * can do, send them an invite, remove them. The list failing to load is how a
 * server without them, or an account that may not see them, is told apart.
 */
class PeopleModel(private val engine: suspend () -> KoanEngineInterface) : ViewModel() {
    private val _state = MutableStateFlow(PeopleState())
    val state: StateFlow<PeopleState> = _state.asStateFlow()

    init {
        viewModelScope.launch { load() }
    }

    fun create(username: String, role: AccountRole, then: (Boolean) -> Unit = {}) {
        val name = username.trim()
        if (name.isEmpty()) return
        change(then) { _state.update { it.copy(invite = engine().createServerAccount(name, role)) } }
    }

    /** An invite for `username`; with `reset`, carrying a new password that signs their devices out. */
    fun invite(username: String, reset: Boolean = false) =
        change { _state.update { it.copy(invite = engine().inviteServerAccount(username, reset)) } }

    fun setPassword(username: String, password: String) = change { engine().setServerAccountPassword(username, password) }

    fun setRole(username: String, role: AccountRole) = change { engine().setServerAccountRole(username, role) }

    fun delete(username: String) = change { engine().deleteServerAccount(username) }

    fun dismissInvite() = _state.update { it.copy(invite = null) }

    /** Run a change and list the accounts again either way, so a refused change shows what is stored. */
    private fun change(then: (Boolean) -> Unit = {}, work: suspend () -> Unit) {
        viewModelScope.launch {
            val ok = try {
                work()
                _state.update { it.copy(error = null) }
                true
            } catch (e: KoanException) {
                _state.update { it.copy(error = e.describe()) }
                false
            }
            load()
            then(ok)
        }
    }

    private suspend fun load() {
        val accounts = try {
            engine().serverAccounts()
        } catch (_: KoanException) {
            null
        }
        _state.update { it.copy(accounts = accounts) }
    }
}

val AccountRole.label
    get() = when (this) {
        AccountRole.READONLY -> "Listen only"
        AccountRole.USER -> "Listen and edit"
        AccountRole.ADMIN -> "Admin"
    }

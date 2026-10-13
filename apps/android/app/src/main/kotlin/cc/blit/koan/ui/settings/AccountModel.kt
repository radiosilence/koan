package cc.blit.koan.ui.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.KoanException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class AccountState(
    /** `null` until the engine has said whether a server is signed in. */
    val signedIn: Boolean? = null,
    val busy: String? = null,
    val error: String? = null,
)

/** Whether a server is signed in to, and signing in. */
class AccountModel(private val mirror: EngineMirror) : ViewModel() {
    private val _state = MutableStateFlow(AccountState())
    val state: StateFlow<AccountState> = _state.asStateFlow()

    init {
        viewModelScope.launch {
            val signedIn = mirror.engine().settings().remoteSignedIn
            _state.update { it.copy(signedIn = signedIn) }
        }
    }

    fun signIn(url: String, username: String, password: String) {
        viewModelScope.launch {
            _state.update { it.copy(busy = "Signing in", error = null) }
            try {
                val engine = mirror.engine()
                engine.signInRemote(url.trim(), username.trim(), password)
                _state.update { it.copy(busy = "Syncing the library") }
                engine.syncRemote()
                _state.update { it.copy(signedIn = true) }
            } catch (e: KoanException) {
                _state.update { it.copy(error = e.describe()) }
            } finally {
                _state.update { it.copy(busy = null) }
            }
        }
    }
}

/** The engine's own words for what went wrong. */
fun KoanException.describe() = when (this) {
    is KoanException.Database -> detail
    is KoanException.Audio -> detail
    is KoanException.Player -> detail
    is KoanException.NotFound -> detail
    is KoanException.BadArgument -> detail
    is KoanException.Remote -> detail
    is KoanException.NeedsSampleRate -> detail
}

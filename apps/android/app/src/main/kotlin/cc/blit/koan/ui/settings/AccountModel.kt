package cc.blit.koan.ui.settings

import android.net.Uri
import androidx.core.net.toUri
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.describe
import cc.blit.koan.ffi.Invite
import cc.blit.koan.ffi.KoanEngineInterface
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.PairingCode
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** A device waiting to be signed in as this account, put to the person before anything is sent. */
data class PairingRequest(
    /** The pairing's id or typed code, as the server knows it. */
    val pair: String,
    val device: String,
    val from: String,
    val local: Boolean,
    val username: String,
    val host: String,
) {
    /** Where the request came from. One from the internet is worth a second look. */
    val origin
        get() = if (local) {
            "Requested from $from, on your network."
        } else {
            "Requested from $from, from the internet. A device in the room with you is usually on your network."
        }
}

data class AccountState(
    /** `null` until the engine has said whether a server is signed in. */
    val signedIn: Boolean? = null,
    val busy: String? = null,
    val error: String? = null,
    /** The code this device shows while it waits for another to approve it. */
    val pairing: PairingCode? = null,
    /** An invite that would replace the account signed in, waiting on a yes. */
    val pendingInvite: Invite? = null,
    /** Another device asking to sign in as this account, waiting on a yes. */
    val pendingPairing: PairingRequest? = null,
    /** What became of something done here that no page is showing. */
    val notice: String? = null,
)

/**
 * Whether a server is signed in to, and every way of getting there: an
 * address and password or API key, an invite link, or a code approved on
 * another device. Also approving another device's code, which signs that
 * device in as this account.
 *
 * Scoped to the activity (`accountModel()`), so the sign-in page, Settings
 * and links opened from outside all share one.
 */
class AccountModel(private val engine: suspend () -> KoanEngineInterface) : ViewModel() {
    private val _state = MutableStateFlow(AccountState())
    val state: StateFlow<AccountState> = _state.asStateFlow()

    private var waiting: Job? = null

    init {
        viewModelScope.launch {
            val signedIn = engine().settings().remoteSignedIn
            _state.update { it.copy(signedIn = signedIn) }
        }
    }

    fun signIn(url: String, username: String, secret: String, withApiKey: Boolean) {
        val server = url.trim()
        signInThen("Signing in to ${host(server)}") {
            if (withApiKey) {
                signInRemoteWithKey(server, username.trim(), secret.trim())
            } else {
                signInRemote(server, username.trim(), secret)
            }
        }
    }

    /** `text` as an invite, if it is one. */
    suspend fun parseInvite(text: String): Invite? = engine().parseInvite(text)

    /** Whatever the system opened koan with: an invite or a pairing link. Anything else is ignored. */
    fun open(uri: Uri) {
        val link = uri.toString()
        viewModelScope.launch {
            val engine = engine()
            val pair = engine.parsePairingLink(link)
            if (pair == null) {
                engine.parseInvite(link)?.let(::offer)
                return@launch
            }
            // Approving signs the device in to this account, so only a link
            // for the server signed in here is answered here.
            val current = engine.settings()
            if (current.remoteSignedIn && current.remoteUrl.trimEnd('/') == pair.server) {
                offerPairing(pair.id)
            } else {
                notice(
                    "A device is asking to sign in to ${host(pair.server)}, which kōan is not signed in to. " +
                        "Approve it on ${host(pair.server)}'s own pair page instead.",
                )
            }
        }
    }

    /** Join with `invite`, asking first when it would replace the account signed in. */
    fun offer(invite: Invite) {
        viewModelScope.launch {
            val current = engine().settings()
            val elsewhere = current.remoteUrl != invite.server || current.remoteUsername != invite.username
            if (current.remoteSignedIn && elsewhere) {
                _state.update { it.copy(pendingInvite = invite) }
            } else {
                join(invite)
            }
        }
    }

    fun join(invite: Invite) {
        _state.update { it.copy(pendingInvite = null) }
        signInThen("Signing in to ${host(invite.server)}") { joinInvite(invite) }
    }

    fun dismissInvite() = _state.update { it.copy(pendingInvite = null) }

    /**
     * Ask the server at `url` to sign this device in once someone signed in
     * elsewhere approves it, and wait. The code and link are shown meanwhile.
     */
    fun startPairing(url: String) {
        val typed = url.trim()
        val server = if (typed.contains("://")) typed else "https://$typed"
        waiting?.cancel()
        waiting = viewModelScope.launch {
            _state.update { it.copy(busy = "Asking ${host(server)}", error = null) }
            val engine = engine()
            try {
                val code = engine.startPairing(server)
                _state.update { it.copy(busy = null, pairing = code) }
                engine.awaitPairing()
                _state.update { it.copy(pairing = null, busy = "Loading the library") }
                syncAfterSignIn()
            } catch (e: KoanException) {
                _state.update { it.copy(pairing = null, error = explainPairing(e)) }
            } finally {
                _state.update { it.copy(busy = null) }
            }
        }
    }

    fun cancelPairing() {
        waiting?.cancel()
        waiting = null
        viewModelScope.launch { engine().cancelPairing() }
        _state.update { it.copy(pairing = null, busy = null) }
    }

    /** Ask the signed-in server which device is waiting on `pair`, an id or a typed code, and put it to the person. */
    fun offerPairing(pair: String) {
        val typed = pair.trim()
        viewModelScope.launch {
            val engine = engine()
            val current = engine.settings()
            try {
                val info = engine.pairingInfo(typed)
                val request = PairingRequest(
                    pair = typed,
                    device = info.device,
                    from = info.from,
                    local = info.local,
                    username = current.remoteUsername,
                    host = host(current.remoteUrl),
                )
                _state.update { it.copy(pendingPairing = request) }
            } catch (e: KoanException) {
                notice("No device is waiting with that code: ${e.describe()}")
            }
        }
    }

    fun settle(request: PairingRequest, approve: Boolean) {
        _state.update { it.copy(pendingPairing = null) }
        viewModelScope.launch {
            val engine = engine()
            try {
                if (approve) {
                    val device = engine.approvePairing(request.pair)
                    notice("$device is signed in as ${request.username}.")
                } else {
                    engine.declinePairing(request.pair)
                }
            } catch (e: KoanException) {
                notice("Could not sign ${request.device} in: ${e.describe()}")
            }
        }
    }

    /** Settings signed out: back to the sign-in page. */
    fun signedOut() = _state.update { it.copy(signedIn = false) }

    fun dismissNotice() = _state.update { it.copy(notice = null) }

    private fun notice(text: String) = _state.update { it.copy(notice = text) }

    private fun signInThen(label: String, signIn: suspend KoanEngineInterface.() -> Unit) {
        viewModelScope.launch {
            _state.update { it.copy(busy = label, error = null) }
            try {
                engine().signIn()
                _state.update { it.copy(busy = "Loading the library") }
                syncAfterSignIn()
            } catch (e: KoanException) {
                _state.update { it.copy(error = e.describe()) }
            } finally {
                _state.update { it.copy(busy = null) }
            }
        }
    }

    /**
     * Signed in: show the library at once, and fill it. A sync that fails
     * leaves the account signed in, with the reason said.
     */
    private suspend fun syncAfterSignIn() {
        _state.update { it.copy(signedIn = true) }
        try {
            engine().syncRemote()
        } catch (e: KoanException) {
            notice("Signed in, but the library did not load: ${e.describe()}")
        }
    }

    private fun explainPairing(e: KoanException): String {
        val reason = e.describe()
        // Only a server without pairing needs the other way in pointed out;
        // a declined or lapsed code is asked for again.
        return if (e is KoanException.NotFound) {
            "${reason.replaceFirstChar { it.uppercase() }}. A server that is not kōan signs in with a password or API key."
        } else {
            "Could not sign in this way: $reason."
        }
    }
}

/** A server's host, for naming it in a sentence. */
fun host(url: String): String = url.toUri().host ?: url

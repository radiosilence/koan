package cc.blit.koan.state

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.Settings

/** Where the server connection stands, for a page that has to say so. */
sealed interface ServerStatus {
    /** The engine has not answered yet. */
    data object Loading : ServerStatus

    /** No server is configured, or sign-in has never happened. */
    data object SignedOut : ServerStatus

    /** The server refused the credential this device holds. */
    data class Refused(val url: String) : ServerStatus

    data class SignedIn(val url: String, val username: String, val linked: Boolean) : ServerStatus
}

/**
 * Settings state over the engine's configuration. Phase 2 holds only what the
 * sign-in state needs; the Settings pages bring the rest, as they did in
 * `SettingsModel.swift`, which this follows: commit on edit, re-read on
 * focus, nothing cached that the engine can say again.
 */
class SettingsModel(
    private val engine: KoanEngineInterface,
    private val mirror: EngineMirror,
    private val scope: CoroutineScope,
) {
    private val _settings = MutableStateFlow<Settings?>(null)
    private val _signingIn = MutableStateFlow(false)
    private val _error = MutableStateFlow<String?>(null)

    val settings: StateFlow<Settings?> = _settings.asStateFlow()
    val signingIn: StateFlow<Boolean> = _signingIn.asStateFlow()
    val error: StateFlow<String?> = _error.asStateFlow()

    /** Read the configuration again. */
    fun refresh() {
        scope.launch {
            try {
                _settings.value = engine.settings()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _error.value = e.message ?: e.toString()
            }
        }
    }

    fun signIn(url: String, username: String, password: String) {
        if (_signingIn.value) return
        _signingIn.value = true
        _error.value = null
        scope.launch {
            try {
                engine.signInRemote(url.trim(), username.trim(), password)
                _settings.value = engine.settings()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _error.value = e.message ?: e.toString()
            } finally {
                _signingIn.value = false
            }
        }
    }

    companion object {
        /** Pure, so a page and a test decide it the same way. */
        fun status(settings: Settings?, refused: Boolean, linked: Boolean): ServerStatus = when {
            settings == null -> ServerStatus.Loading
            !settings.remoteEnabled || settings.remoteUrl.isEmpty() || !settings.remoteSignedIn ->
                ServerStatus.SignedOut
            refused -> ServerStatus.Refused(settings.remoteUrl)
            else -> ServerStatus.SignedIn(settings.remoteUrl, settings.remoteUsername, linked)
        }
    }

    /** The status as the mirror's connection slice reads now. */
    fun status(): ServerStatus = status(
        _settings.value,
        mirror.signInRefused,
        mirror.connection.value?.linked == true,
    )
}

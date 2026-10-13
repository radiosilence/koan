package cc.blit.koan.ui.settings

import android.text.format.Formatter
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.ConnectedInfo
import cc.blit.koan.ffi.HeldInfo
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.ScrobblingConnection
import cc.blit.koan.ffi.ServerQueue
import cc.blit.koan.ffi.Settings
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** The ListenBrainz connection as far as it is known: asked once per page. */
sealed interface Scrobbling {
    data object Checking : Scrobbling

    /** The server could not say, so neither state is shown. */
    data object Unknown : Scrobbling

    data class Known(val connection: ScrobblingConnection?) : Scrobbling
}

data class SettingsState(
    /** `null` until the engine has answered. */
    val settings: Settings? = null,
    /** What the last thing done here came to. Gone after a few seconds. */
    val result: String? = null,
    /** Why the last thing done here failed. Stays until the next action. */
    val error: String? = null,
    /** Things running, by what they hold, so a second sync waits for the first. */
    val running: Set<String> = emptySet(),
    val scrobbling: Scrobbling = Scrobbling.Checking,
    /** What the server holds, while asking before it replaces this device's queue. */
    val replacingQueue: ServerQueue? = null,
)

/**
 * Settings' copy of the configuration, the counterpart of the Apple apps'
 * `SettingsModel.swift`.
 *
 * Read when the page opens and again when the app returns to the front, and
 * written whole through `KoanEngine.updateSettings` as each control changes:
 * the engine routes each key to the file that owns it.
 */
class SettingsModel(private val mirror: EngineMirror) : ViewModel() {
    private val _state = MutableStateFlow(SettingsState())
    val state: StateFlow<SettingsState> = _state.asStateFlow()

    private var dismissal: Job? = null

    init {
        reload()
    }

    fun reload() {
        viewModelScope.launch {
            val settings = mirror.engine().settings()
            _state.update { it.copy(settings = settings) }
        }
    }

    /** Change a field and write the result. Every control commits through here. */
    fun edit(change: (Settings) -> Settings) {
        val current = _state.value.settings ?: return
        val next = change(current)
        _state.update { it.copy(settings = next) }
        viewModelScope.launch {
            try {
                mirror.engine().updateSettings(next)
                _state.update { it.copy(error = null) }
            } catch (e: KoanException) {
                failed(e)
            }
        }
    }

    // -- Server ----------------------------------------------------------

    fun syncNow() = run(holds = REMOTE) {
        val s = syncRemote()
        // "0 tracks across 0 albums" reads as a failure, and is what an
        // unchanged library answers.
        if (s.tracks == 0u && s.favouritesImported == 0u) {
            "Already up to date"
        } else {
            "${s.tracks.toLong().formatted()} tracks across ${s.albums.toLong().formatted()} albums"
        }
    }

    fun signOut(forgetTracks: Boolean, then: () -> Unit) {
        viewModelScope.launch {
            val engine = mirror.engine()
            try {
                engine.signOutRemote()
            } catch (e: KoanException) {
                failed(e)
                reload()
                return@launch
            }
            if (forgetTracks) {
                // Local rows too: it takes the server off the ones that were
                // on both.
                try {
                    engine.forgetRemote()
                } catch (e: KoanException) {
                    failed(e)
                }
            }
            then()
        }
    }

    fun clearCache(context: android.content.Context) = run(holds = DOWNLOADS) {
        val c = clearDownloadCache()
        "Freed ${Formatter.formatShortFileSize(context, c.bytes.toLong())} across ${c.files} files"
    }

    fun setOffline(on: Boolean) = mirror.launch { setOffline(on) }

    /** Change the signed-in account's password. This device stays signed in. */
    fun changePassword(current: String, new: String, then: (Boolean) -> Unit) {
        viewModelScope.launch {
            try {
                mirror.engine().changeOwnPassword(current, new)
                done("Password changed. Your other devices will have to sign in again.")
                then(true)
            } catch (e: KoanException) {
                failed(e)
                then(false)
            }
        }
    }

    /** Keep this device's queue on the server: asked first when the server already holds one. */
    fun setServerQueue(on: Boolean) {
        viewModelScope.launch {
            val engine = mirror.engine()
            try {
                if (on) {
                    val saved = engine.serverQueue()
                    if (saved.savedTracks > 0u) {
                        _state.update { it.copy(replacingQueue = saved) }
                        return@launch
                    }
                }
                engine.setServerQueue(on)
                _state.update { it.copy(error = null) }
            } catch (e: KoanException) {
                failed(e)
            }
            reload()
        }
    }

    fun confirmServerQueue(replace: Boolean) {
        _state.update { it.copy(replacingQueue = null) }
        if (!replace) return
        viewModelScope.launch {
            try {
                mirror.engine().setServerQueue(true)
            } catch (e: KoanException) {
                failed(e)
            }
            reload()
        }
    }

    // -- Devices ---------------------------------------------------------

    /** Let `account` on this server control this device, or stop. */
    fun shareDevice(account: String, allow: Boolean) {
        viewModelScope.launch {
            try {
                mirror.engine().shareDevice(account.trim(), allow)
            } catch (e: KoanException) {
                failed(e)
            }
        }
    }

    /** Hang up a device connected on the network; refusing it keeps its address out too. */
    fun endConnection(connection: ConnectedInfo, refuse: Boolean) {
        val addr = connection.addr
        if (refuse && addr != null) {
            edit { s -> if (addr in s.devicesRefused) s else s.copy(devicesRefused = s.devicesRefused + addr) }
        }
        connection.key?.let { key -> mirror.launch { endConnection(key) } }
    }

    fun allowHeld(held: HeldInfo) = mirror.launch { allowHeld(held.addr) }

    // -- Scrobbling ------------------------------------------------------

    fun loadScrobbling() {
        viewModelScope.launch {
            try {
                val c = mirror.engine().scrobblingStatus()
                _state.update { it.copy(scrobbling = Scrobbling.Known(c)) }
            } catch (e: KoanException) {
                _state.update { it.copy(scrobbling = Scrobbling.Unknown) }
                failed(e)
            }
        }
    }

    fun connectScrobbling(token: String, then: () -> Unit) = run(holds = SCROBBLING) {
        val c = connectScrobbling(token.trim())
        _state.update { it.copy(scrobbling = Scrobbling.Known(c)) }
        then()
        null
    }

    fun disconnectScrobbling() = run(holds = SCROBBLING) {
        disconnectScrobbling()
        _state.update { it.copy(scrobbling = Scrobbling.Known(null)) }
        null
    }

    // -- Running ---------------------------------------------------------

    fun isRunning(holds: String) = holds in _state.value.running

    /** Run `block` unless something holding the same thing is; say what it came to. */
    private fun run(holds: String, block: suspend KoanEngine.() -> String?) {
        if (holds in _state.value.running) return
        _state.update { it.copy(running = it.running + holds, error = null) }
        viewModelScope.launch {
            try {
                mirror.engine().block()?.let(::done)
            } catch (e: KoanException) {
                failed(e)
            } finally {
                _state.update { it.copy(running = it.running - holds) }
                reload()
            }
        }
    }

    private fun done(result: String) {
        _state.update { it.copy(result = result, error = null) }
        dismissal?.cancel()
        dismissal = viewModelScope.launch {
            delay(6_000)
            _state.update { if (it.result == result) it.copy(result = null) else it }
        }
    }

    private fun failed(e: KoanException) = _state.update { it.copy(error = e.describe(), result = null) }

    companion object {
        const val REMOTE = "remote"
        const val DOWNLOADS = "downloads"
        const val SCROBBLING = "scrobbling"
    }
}

private fun Long.formatted() = "%,d".format(this)

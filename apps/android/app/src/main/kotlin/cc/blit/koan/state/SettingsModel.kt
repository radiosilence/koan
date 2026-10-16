package cc.blit.koan.state

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import uniffi.koan_ffi.Appearance
import uniffi.koan_ffi.ConnectedInfo
import uniffi.koan_ffi.Invite
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.KoanException
import uniffi.koan_ffi.OutputChoice
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

/** A device asking to be signed in, as the signed-in server describes it, waiting on the person's answer. */
data class PairingRequest(
    /** The pairing's id or code, as the server knows it. */
    val pair: String,
    val device: String,
    /** The address the request came from. */
    val from: String,
    /** The address is on a private network. */
    val local: Boolean,
    val username: String,
    val host: String,
) {
    /** Where the request came from, in plain words. One from the internet is worth a second look. */
    val origin: String
        get() = if (local) {
            "Requested from $from, on your network."
        } else {
            "Requested from $from, from the internet. A device in the room with you is usually on your network."
        }
}

/** This device waiting to be signed in by another one. */
sealed interface PairingState {
    data object Idle : PairingState

    /** The server has been asked for a code. */
    data object Starting : PairingState

    /** The code and link to show; the answer is awaited. */
    data class Waiting(val code: String, val link: String) : PairingState

    data class Failed(val message: String) : PairingState
}

/**
 * Settings state over the engine's configuration, `SettingsModel.swift` in
 * Kotlin: the configuration is read when the page opens and again whenever it
 * regains focus ([refresh]), and written when a field is committed, not on
 * every keystroke. `config.toml` is shared with the CLI and the TUI, and
 * rewriting it while someone types a URL means a half-typed value is briefly
 * the live configuration for anything reading it.
 *
 * Every method returns at once and works on [scope] (the engine's dispatcher).
 * Nothing here holds a secret longer than the call that needs it: a password
 * or API key is passed to the engine and forgotten, and no message built here
 * contains one.
 *
 * Methods that start slow library work take the [ActivityModel], so the work
 * shows as a row and the buttons that would collide with it grey out. The
 * runtime builds this model without it, hence a parameter per call.
 */
class SettingsModel(
    private val engine: KoanEngineInterface,
    private val mirror: EngineMirror,
    private val scope: CoroutineScope,
    /** How long a result stays up before it clears itself. An error stays until the next action. */
    private val resultMillis: Long = 6_000,
) {
    private val _settings = MutableStateFlow<Settings?>(null)
    private val _appearance = MutableStateFlow<Appearance?>(null)
    private val _signingIn = MutableStateFlow(false)
    private val _error = MutableStateFlow<String?>(null)
    private val _result = MutableStateFlow<String?>(null)
    private val _pairing = MutableStateFlow<PairingState>(PairingState.Idle)
    private val _pendingPairing = MutableStateFlow<PairingRequest?>(null)
    private val _pendingInvite = MutableStateFlow<Invite?>(null)
    private val _cacheLimitRejected = MutableStateFlow(false)

    /** Serialises writes, so the last edit is the last write however the engine's pool orders them. */
    private val writes = Mutex()

    @Volatile
    private var pairingRun = 0L

    val settings: StateFlow<Settings?> = _settings.asStateFlow()

    /** `[appearance]`, read with the settings. */
    val appearance: StateFlow<Appearance?> = _appearance.asStateFlow()
    val signingIn: StateFlow<Boolean> = _signingIn.asStateFlow()

    /** What the last action failed with. Stays until the next action. */
    val error: StateFlow<String?> = _error.asStateFlow()

    /** What the last action came to; clears itself. */
    val result: StateFlow<String?> = _result.asStateFlow()

    /** This device's own pairing, while it waits for another to approve it. */
    val pairing: StateFlow<PairingState> = _pairing.asStateFlow()

    /** A device waiting for the person to allow it in, from a typed code or a link. */
    val pendingPairing: StateFlow<PairingRequest?> = _pendingPairing.asStateFlow()

    /** An invite to a different account than the one signed in, waiting to be confirmed. */
    val pendingInvite: StateFlow<Invite?> = _pendingInvite.asStateFlow()

    /** The cache limit field's text was refused by [SettingsRules.cacheLimit] and nothing was written. */
    val cacheLimitRejected: StateFlow<Boolean> = _cacheLimitRejected.asStateFlow()

    // -- Reading ------------------------------------------------------------------------------

    /**
     * Read the configuration again. A page calls this when it opens and on every
     * resume, so a change made elsewhere (the CLI, another window) shows up
     * instead of being silently overwritten by what this one last saw.
     */
    fun refresh() {
        scope.launch { reload() }
    }

    private suspend fun reload() {
        writes.withLock {
            try {
                _settings.value = engine.settings()
                _appearance.value = engine.appearance()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _error.value = describe(e)
            }
        }
    }

    // -- Editing ------------------------------------------------------------------------------

    /**
     * Change a field and write the result. Every control commits through here,
     * so there is one place that decides when the file is touched. The state is
     * updated at once and the write follows; a write that fails shows its error
     * and the next [refresh] shows what the engine actually holds.
     */
    fun edit(change: (Settings) -> Settings) {
        _settings.updateAndGet { it?.let(change) } ?: return
        scope.launch { commit() }
    }

    private suspend fun commit() {
        writes.withLock {
            // Whatever is latest when the lock comes, not the value this call was made for.
            val latest = _settings.value ?: return
            try {
                engine.updateSettings(latest)
                _error.value = null
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _error.value = describe(e)
            }
        }
    }

    fun setAutoSync(on: Boolean) = edit { it.copy(autoSync = on) }

    fun setAutoSyncInterval(minutes: Long) = edit { it.copy(autoSyncIntervalMins = minutes.coerceAtLeast(0).toULong()) }

    fun setDownloadWorkers(n: Int) = edit { it.copy(downloadWorkers = SettingsRules.workers(n)) }

    /**
     * Commit the cache limit as typed. Blank is no limit; anything else must be
     * a size ([SettingsRules.cacheLimit]), and one that is not writes nothing and
     * raises [cacheLimitRejected]. Returns whether it was accepted.
     */
    fun setCacheLimit(typed: String): Boolean {
        val limit = SettingsRules.cacheLimit(typed)
        _cacheLimitRejected.value = limit == null
        if (limit == null) {
            _error.value = "That isn't a size. Try 50GB, or leave it blank for no limit."
            return false
        }
        if (_settings.value?.cacheLimit != limit) edit { it.copy(cacheLimit = limit) }
        return true
    }

    fun setFadeOnPause(on: Boolean) = edit { it.copy(fadeOnPause = on) }

    /** `off`, `track` or `album`. */
    fun setReplayGain(mode: String) = edit { it.copy(replaygain = mode) }

    fun setPreAmp(db: Double) = edit { it.copy(preAmpDb = SettingsRules.preAmp(db)) }

    fun setDiscoverable(on: Boolean) = edit { it.copy(devicesDiscoverable = on) }

    /** `full` or `playback`. */
    fun setNearbyControl(mode: String) = edit { it.copy(devicesNearbyControl = mode) }

    /** Add a device to dial by address. Returns false, writing nothing, for something that is not one. */
    fun addAddress(typed: String): Boolean {
        val address = SettingsRules.address(typed)
        if (address == null) {
            _error.value = "That isn't an address. Use a host or host:port."
            return false
        }
        edit { s -> if (address in s.devicesAddresses) s else s.copy(devicesAddresses = s.devicesAddresses + address) }
        return true
    }

    fun removeAddress(address: String) = edit { it.copy(devicesAddresses = it.devicesAddresses - address) }

    /** Let a refused address connect again. */
    fun allowRefused(address: String) = edit { it.copy(devicesRefused = it.devicesRefused - address) }

    /**
     * Hang up a device connected to this one on the network. Refusing it as
     * well keeps its address from connecting again.
     */
    fun endConnection(connection: ConnectedInfo, refuse: Boolean) {
        val addr = connection.addr
        if (refuse && addr != null) edit { s -> if (addr in s.devicesRefused) s else s.copy(devicesRefused = s.devicesRefused + addr) }
        val key = connection.key ?: return
        scope.launch { engine.endConnection(key) }
    }

    /** Let a disconnected device back in. */
    fun allowHeld(addr: String) {
        scope.launch { engine.allowHeld(addr) }
    }

    /** Offline mode: only what is on this phone. */
    fun setOffline(on: Boolean) {
        scope.launch { engine.setOffline(on) }
    }

    // -- Appearance ---------------------------------------------------------------------------

    /** Colours from the record, or kōan's mint. Saved at once; the engine holds the value, the theme reads it. */
    fun setRecordColours(on: Boolean) {
        _appearance.update { it?.copy(recordColours = on) }
        scope.launch { engine.setRecordColours(on) }
    }

    // -- Output -------------------------------------------------------------------------------

    fun selectOutput(choice: OutputChoice) {
        scope.launch {
            try {
                engine.selectOutput(choice)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _error.value = describe(e)
            }
        }
    }

    /** Ask the network for renderers; they arrive through the mirror. */
    fun searchRenderers() {
        scope.launch { engine.searchRenderers() }
    }

    fun refreshOutputs() {
        scope.launch { engine.refreshOutputs() }
    }

    // -- Library actions ----------------------------------------------------------------------

    /** Scan the configured folders. A phone usually has none (on-device scanning is not part of the Android app yet). */
    fun scan(activity: ActivityModel? = null, force: Boolean = false) {
        scope.launch {
            val label = if (force) "Rescanning every file" else "Scanning library"
            val outcome = if (activity != null) {
                activity.runReporting(label, Resource.LocalLibrary) { progress -> engine.scanReporting(force, progress) }
            } else {
                runCatchingCancellable { engine.scanReporting(force, null) }
            }
            outcome.fold(
                { s -> say("${s.added} added, ${s.updated} updated, ${s.removed} removed") },
                { fail(it) },
            )
            reload()
        }
    }

    /**
     * Stop scanning a folder, and optionally forget what it put in the library.
     * Keeping the rows leaves records on screen whose files koan will never look
     * at again; forgetting them is what makes "remove every folder and sign out"
     * end at an empty library.
     */
    fun removeFolder(path: String, forgetTracks: Boolean, activity: ActivityModel? = null) {
        edit { s -> s.copy(libraryFolders = s.libraryFolders.filterNot { it.path == path }) }
        if (!forgetTracks) return
        scope.launch {
            val outcome = tracked(activity, "Forgetting ${path.substringAfterLast('/')}", setOf(Resource.LocalTracks)) { engine.forgetFolder(path) }
            outcome.fold({ say("Forgot ${it.toLong()} tracks") }, { fail(it) })
            reload()
        }
    }

    fun syncNow(activity: ActivityModel? = null) {
        scope.launch {
            val outcome = tracked(activity, "Syncing with server", setOf(Resource.RemoteTracks), followsSync = true) { engine.syncRemote() }
            outcome.fold(
                { s ->
                    // "0 tracks across 0 albums" reads as a failure, and is what an empty server answers.
                    say(
                        if (s.tracks == 0u && s.favouritesImported == 0u) "Already up to date"
                        else "${s.tracks} tracks across ${s.albums} albums",
                    )
                },
                { fail(it) },
            )
            reload()
        }
    }

    fun clearCache(activity: ActivityModel? = null) {
        scope.launch {
            val outcome = tracked(activity, "Clearing downloads", setOf(Resource.Downloads)) { engine.clearDownloadCache() }
            outcome.fold(
                { c -> say("Freed ${SettingsRules.bytes(c.bytes.toLong())} across ${c.files} files") },
                { fail(it) },
            )
            reload()
        }
    }

    fun rebuildIndex(activity: ActivityModel? = null) {
        scope.launch {
            val outcome = tracked(activity, "Clearing the library index", Resource.WholeLibrary) { engine.rebuildIndex() }
            outcome.fold({ say("Removed ${it.tracks} tracks. Sync to rebuild.") }, { fail(it) })
            reload()
        }
    }

    // -- Signing in ---------------------------------------------------------------------------

    /**
     * Sign in with a password, or with an API key made in the web UI (one that
     * can be revoked on its own). Checked against the server before anything
     * is written. A fresh sign-in then syncs, so the person lands on a library.
     */
    fun signIn(
        url: String,
        username: String,
        secret: String,
        withApiKey: Boolean = false,
        activity: ActivityModel? = null,
    ) {
        if (_signingIn.value) return
        if (url.isBlank() || username.isBlank() || secret.isEmpty()) {
            _error.value = "Enter the server, the username and the ${if (withApiKey) "API key" else "password"}."
            return
        }
        _signingIn.value = true
        _error.value = null
        scope.launch {
            try {
                val outcome = tracked(activity, "Signing in") {
                    if (withApiKey) engine.signInRemoteWithKey(url.trim(), username.trim(), secret)
                    else engine.signInRemote(url.trim(), username.trim(), secret)
                }
                outcome.fold({ say("Signed in to ${url.trim()}") }, { fail(it) })
                reload()
                if (outcome.isSuccess) syncAfterSignIn(activity)
            } finally {
                _signingIn.value = false
            }
        }
    }

    private suspend fun syncAfterSignIn(activity: ActivityModel?) {
        tracked(activity, "Loading the library", setOf(Resource.RemoteTracks), followsSync = true) { engine.syncRemote() }
            .onFailure { fail(it) }
    }

    fun signOut(forgetTracks: Boolean, activity: ActivityModel? = null) {
        scope.launch {
            try {
                engine.signOutRemote()
                say("Signed out")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                fail(e)
                reload()
                return@launch
            }
            if (forgetTracks) {
                // Local rows too: it takes the server off the ones that were on both.
                val outcome = tracked(activity, "Forgetting the server's tracks", setOf(Resource.RemoteTracks, Resource.LocalTracks)) { engine.forgetRemote() }
                outcome.fold({ say("Signed out and forgot ${it.toLong()} tracks") }, { fail(it) })
            }
            reload()
        }
    }

    // -- Invites ------------------------------------------------------------------------------

    /**
     * Whatever was pasted or opened, offered as an invite. Not one says so. One
     * for the account already signed in, or any when signed out, joins at once;
     * one for another account waits for [confirmInvite], because replacing an
     * account is not a thing to do to someone unasked. [quiet] is for a field
     * probing every edit: not an invite is then not an error.
     */
    fun offerInvite(text: String, activity: ActivityModel? = null, quiet: Boolean = false) {
        scope.launch {
            val invite = try {
                engine.parseInvite(text.trim())
            } catch (e: Exception) {
                null
            }
            if (invite == null) {
                if (!quiet) _error.value = "That isn't a kōan invite."
                return@launch
            }
            val current = engine.settings()
            val elsewhere = current.remoteUrl != invite.server || current.remoteUsername != invite.username
            if (current.remoteSignedIn && elsewhere) _pendingInvite.value = invite else join(invite, activity)
        }
    }

    fun confirmInvite(activity: ActivityModel? = null) {
        val invite = _pendingInvite.value ?: return
        _pendingInvite.value = null
        scope.launch { join(invite, activity) }
    }

    fun dismissInvite() {
        _pendingInvite.value = null
    }

    private suspend fun join(invite: Invite, activity: ActivityModel?) {
        _signingIn.value = true
        _error.value = null
        val host = hostOf(invite.server)
        try {
            val outcome = tracked(activity, "Signing in to $host") { engine.joinInvite(invite) }
            outcome.fold({ say("Signed in to $host") }, { _error.value = "Could not sign in to $host: ${describe(it)}" })
            reload()
            if (outcome.isSuccess) syncAfterSignIn(activity)
        } finally {
            _signingIn.value = false
        }
    }

    // -- Pairing: this device waiting to be signed in ------------------------------------------

    /**
     * Ask the server at [url] to sign this device in once someone signed in
     * elsewhere approves it. The code and link to show arrive as
     * [PairingState.Waiting]; approval signs the device in and the shell's gate
     * lifts by itself when the settings are read again.
     */
    fun startPairing(url: String) {
        if (url.isBlank()) {
            _error.value = "Enter the server's address."
            return
        }
        val run = ++pairingRun
        _pairing.value = PairingState.Starting
        _error.value = null
        scope.launch {
            try {
                val code = engine.startPairing(url.trim())
                if (run != pairingRun) return@launch
                _pairing.value = PairingState.Waiting(code.code, code.link)
                engine.awaitPairing()
                if (run != pairingRun) return@launch
                _pairing.value = PairingState.Idle
                reload()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (run == pairingRun) _pairing.value = PairingState.Failed(describe(e))
            }
        }
    }

    /** Give up the pairing this device is waiting on. */
    fun cancelPairing() {
        pairingRun++
        _pairing.value = PairingState.Idle
        scope.launch { engine.cancelPairing() }
    }

    // -- Pairing: approving another device -----------------------------------------------------

    /**
     * Ask the signed-in server which device waits on [typed], a code as shown
     * (forgiven its case and its dash), and put the question to the person.
     */
    fun offerPairing(typed: String) {
        val pair = PairCode.normalise(typed) ?: typed.trim().takeIf { it.isNotEmpty() && it.none(Char::isWhitespace) }
        if (pair == null) {
            _error.value = "Enter the code the other device shows."
            return
        }
        scope.launch { askPairing(pair) }
    }

    /** A `koan.rocks/pair/` link opened from outside. Only for the server signed in here. */
    fun offerPairingLink(link: String) {
        scope.launch {
            val parsed = engine.parsePairingLink(link.trim())
            if (parsed == null) {
                _error.value = "That isn't a pairing link."
                return@launch
            }
            val current = engine.settings()
            val signedIn = current.remoteUrl.trimEnd('/')
            if (!current.remoteSignedIn || signedIn != parsed.server.trimEnd('/')) {
                _error.value = "A device is asking to sign in to ${hostOf(parsed.server)}, which kōan is not signed in to. " +
                    "Approve it on that server's own pair page instead."
                return@launch
            }
            askPairing(parsed.id)
        }
    }

    private suspend fun askPairing(pair: String) {
        try {
            val current = engine.settings()
            val info = engine.pairingInfo(pair)
            _pendingPairing.value = PairingRequest(
                pair = pair, device = info.device, from = info.from, local = info.local,
                username = current.remoteUsername, host = hostOf(current.remoteUrl),
            )
            _error.value = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            _error.value = "No device is waiting with that code: ${describe(e)}"
        }
    }

    /** The person's answer to [pendingPairing]. */
    fun settlePairing(approve: Boolean) {
        val request = _pendingPairing.value ?: return
        _pendingPairing.value = null
        scope.launch {
            try {
                if (approve) {
                    val device = engine.approvePairing(request.pair)
                    say("$device is signed in as ${request.username}.")
                } else {
                    engine.declinePairing(request.pair)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _error.value = "Could not sign ${request.device} in: ${describe(e)}"
            }
        }
    }

    // -- Messages -----------------------------------------------------------------------------

    /** Show a message of the page's own, as an error. */
    fun report(message: String) {
        _error.value = message
    }

    /** Clear the error, for a page that has shown it. */
    fun clearError() {
        _error.value = null
    }

    private fun say(message: String) {
        _error.value = null
        _result.value = message
        scope.launch {
            delay(resultMillis)
            _result.update { if (it == message) null else it }
        }
    }

    private fun fail(error: Throwable) {
        _error.value = describe(error)
    }

    private suspend fun <T> tracked(
        activity: ActivityModel?,
        label: String,
        uses: Set<Resource> = emptySet(),
        followsSync: Boolean = false,
        work: suspend () -> T,
    ): Result<T> =
        if (activity != null) activity.run(label, uses, followsSync, work) else runCatchingCancellable(work)

    private suspend fun <T> runCatchingCancellable(work: suspend () -> T): Result<T> = try {
        Result.success(work())
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        Result.failure(e)
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

        /** The host of a server address, or the address itself when it will not parse. */
        fun hostOf(server: String): String =
            runCatching { java.net.URI(server.trim()).host }.getOrNull()?.takeIf { it.isNotEmpty() } ?: server

        /**
         * Engine errors carry a message worth reading; the exception's own
         * rendering wraps it (`detail=...`). Secrets never reach here: the
         * engine's messages name the server and the problem, not the credential.
         */
        fun describe(error: Throwable): String = when (error) {
            // A reflective caller (a proxy over the engine, as the tests use) wraps what the engine threw.
            is java.lang.reflect.UndeclaredThrowableException -> describe(error.undeclaredThrowable)
            is KoanException.BadArgument -> error.detail
            is KoanException.Database -> error.detail
            is KoanException.NotFound -> error.detail
            is KoanException.Audio -> error.detail
            is KoanException.Player -> error.detail
            is KoanException.Remote -> error.detail
            is KoanException.NeedsSampleRate -> error.detail
            else -> error.message ?: error.toString()
        }
    }

    /** The status as the mirror's connection slice reads now. */
    fun status(): ServerStatus = status(
        _settings.value,
        mirror.signInRefused,
        mirror.connection.value?.linked == true,
    )
}

private inline fun <T> MutableStateFlow<T>.updateAndGet(function: (T) -> T): T {
    while (true) {
        val prev = value
        val next = function(prev)
        if (compareAndSet(prev, next)) return next
    }
}

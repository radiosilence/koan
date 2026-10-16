package cc.blit.koan.engine

import android.content.Context
import android.os.Build
import android.util.Log
import cc.blit.koan.Native
import cc.blit.koan.service.BitPerfect
import cc.blit.koan.service.BitPerfectStatus
import cc.blit.koan.service.Grant
import cc.blit.koan.service.OutputRoute
import cc.blit.koan.art.Artwork
import cc.blit.koan.art.ArtworkStore
import cc.blit.koan.nav.Navigator
import cc.blit.koan.state.ActivityModel
import cc.blit.koan.state.EngineMirror
import cc.blit.koan.state.KeyValueStore
import cc.blit.koan.state.LibraryModel
import cc.blit.koan.state.PlayerModel
import cc.blit.koan.state.PlaylistsModel
import cc.blit.koan.state.SearchModel
import cc.blit.koan.state.SettingsModel
import cc.blit.koan.theme.KoanAccent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import uniffi.koan_ffi.KoanEngine

/** The engine and everything that reads it, built once. */
class KoanRuntime(
    val engine: KoanEngine,
    val mirror: EngineMirror,
    val player: PlayerModel,
    val settings: SettingsModel,
    val library: LibraryModel,
    val playlists: PlaylistsModel,
    val activity: ActivityModel,
    val navigator: Navigator,
    val search: SearchModel,
    val artwork: ArtworkStore,
    /** The accent of the record playing; see [ModelWiring.accent]. */
    val accent: StateFlow<KoanAccent>,
)

/**
 * The process's one engine.
 *
 * The playback service creates it and is the only thing that does: an
 * activity never starts the engine, so leaving or rotating the app does not
 * touch playback. It is a process singleton, not the service's field,
 * because the engine is one per process (it sets process-wide configuration
 * and starts the player thread) and the service may be stopped and started
 * again within one process, after the "stay reachable" window or a swipe
 * from Recents. A second `withAppDirs` in that process is not something this
 * code has any way to know is safe, so there is not one.
 *
 * Everything here runs on [EngineDispatcher], never Main. TLS is initialised
 * first, there, because `Native` loads the library and loading it on Main is
 * a disk read.
 */
object KoanProcess {
    private const val TAG = "KoanProcess"

    private val scope = CoroutineScope(SupervisorJob() + EngineDispatcher.dispatcher)
    private val _runtime = MutableStateFlow<KoanRuntime?>(null)
    private val _failure = MutableStateFlow<String?>(null)
    private val _bitPerfect = MutableStateFlow<BitPerfectStatus>(BitPerfectStatus.No("Not playing"))
    private var started = false
    private var route: OutputRoute? = null

    /** Null until the engine is up. */
    val runtime: StateFlow<KoanRuntime?> = _runtime.asStateFlow()

    /** Why the engine could not start, if it could not. */
    val failure: StateFlow<String?> = _failure.asStateFlow()

    /**
     * Whether the route is bit-perfect, judged from what the output stream
     * was granted. Updated after a route change and when the source's format
     * moves; see [refreshBitPerfect] for how late that can be.
     */
    val bitPerfect: StateFlow<BitPerfectStatus> = _bitPerfect.asStateFlow()

    /** Start the engine, once. Safe to call from any thread, any number of times. */
    @Synchronized
    fun ensureStarted(context: Context) {
        if (started) return
        started = true
        val app = context.applicationContext
        val files = app.filesDir.path
        val cache = app.cacheDir.path
        scope.launch {
            try {
                Native.initTlsOnce(app)
                val engine = KoanEngine.withAppDirs(Build.MODEL, files, cache)
                val mirror = EngineMirror()
                val player = PlayerModel(engine, mirror, scope)
                val settings = SettingsModel(engine, mirror, scope)
                val artwork = Artwork.store
                val activity = ActivityModel().also { it.cancelLibraryTask = engine::cancelLibraryTask }
                val store = KeyValueStore.Prefs(app)
                val library = LibraryModel(engine, mirror, scope, store, activity, artwork)
                val playlists = PlaylistsModel(engine, mirror, scope)
                val navigator = Navigator(scope, AppPageLoader(library, playlists))
                val search = SearchModel(engine, mirror, player, navigator, scope)
                val wiring = ModelWiring(scope, mirror, library, playlists, activity, artwork)
                scope.launch { mirror.follow(engine.observe()) }
                player.start()
                settings.refresh()
                wiring.start()
                _runtime.value = KoanRuntime(
                    engine, mirror, player, settings, library, playlists, activity, navigator, search, artwork, wiring.accent,
                )
                route?.let { announce(engine, it, 0) }
                player.restoreSession()
            } catch (e: Throwable) {
                Log.e(TAG, "engine did not start", e)
                _failure.value = e.message ?: e.toString()
            }
        }
    }

    /**
     * The route the system plays music to. Tells the engine, which names it
     * for its profiles and reopens the output on it. Before the engine is up
     * the latest is held and told once it is.
     */
    fun setRoute(route: OutputRoute, bitPerfectRate: Int = 0) {
        this.route = route
        val runtime = _runtime.value ?: return
        scope.launch { announce(runtime.engine, route, bitPerfectRate) }
    }

    /**
     * `bitPerfectRate` is the rate the mixer was put at, when it was put in
     * bit-perfect mode for this route: what a shared stream then runs at.
     */
    private suspend fun announce(engine: KoanEngine, route: OutputRoute, bitPerfectRate: Int) {
        val rate = if (bitPerfectRate > 0) bitPerfectRate else route.mixerRate
        engine.setAudioRouteAndroid(route.name, route.deviceId, rate.coerceAtLeast(0).toUInt())
        refreshBitPerfect(bitPerfectRate > 0)
    }

    /**
     * Read what the output was granted and say whether it is bit-perfect.
     *
     * The stream opens on the engine's schedule, shortly after a track's
     * format is known or a route changes, and there is no signal for "now it
     * is open" across the FFI. So this waits a moment before reading; a read
     * that comes too early reports the previous stream until the next
     * refresh. Whether that delay is long enough on real hardware is not
     * known.
     */
    fun refreshBitPerfect(mixerBitPerfect: Boolean) {
        val runtime = _runtime.value ?: return
        val route = route
        scope.launch {
            delay(GRANT_SETTLE_MS)
            val grant = runtime.engine.audioOutputGrant()?.let {
                Grant(it.exclusive, it.streamRate.toInt(), it.hardwareRate.toInt(), it.deviceId)
            }
            val sourceRate = runtime.mirror.playback.value.format?.sampleRate?.toInt() ?: 0
            _bitPerfect.value = BitPerfect.status(route, grant, sourceRate, mixerBitPerfect)
        }
    }

    /** Playback is going away with its service: remember where it was, and let any renderer go. */
    fun saveAndRelease() {
        val runtime = _runtime.value ?: return
        scope.launch {
            runCatching { runtime.player.saveOnLeaving() }
            runtime.engine.releaseOutput()
        }
    }

    private const val GRANT_SETTLE_MS = 400L
}

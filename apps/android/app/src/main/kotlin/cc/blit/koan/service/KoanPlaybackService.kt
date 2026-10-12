package cc.blit.koan.service

import android.app.PendingIntent
import android.content.Intent
import android.media.AudioManager
import android.os.Looper
import androidx.media3.common.Player
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import cc.blit.koan.MainActivity
import cc.blit.koan.engine.KoanProcess
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.state.PlaybackControls
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.launch
import uniffi.koan_ffi.PlayState

/**
 * The reason playback survives the app being left: a Media3
 * `MediaSessionService` in the foreground with type `mediaPlayback`, so the
 * system keeps the process while music plays and draws the notification,
 * lock-screen controls and headset-button handling from the session.
 *
 * It starts the engine ([KoanProcess]) and is the only thing that does, then
 * stays out of the way: [KoanPlayer] reads the mirror and forwards commands,
 * and this class adds what Android asks of a music app that the engine
 * cannot know: audio focus, becoming noisy, the route, and bit-perfect USB.
 *
 * Main thread throughout (the framework calls here), so nothing here calls
 * the engine except by [KoanProcess], which hops to the engine dispatcher.
 */
class KoanPlaybackService : MediaSessionService() {
    private val main = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var session: MediaSession? = null
    private var runtime: KoanRuntime? = null
    private var controls: PlaybackControls? = null
    private lateinit var player: KoanPlayer
    private lateinit var focus: AudioFocusController
    private lateinit var noisy: NoisyReceiver
    private lateinit var routes: RouteWatcher
    private lateinit var usb: UsbBitPerfect

    /** The route's device, for re-asking the mixer when the source's rate changes. */
    private var routeDevice: android.media.AudioDeviceInfo? = null
    private var stopTimer: Job? = null

    /** Foreground is kept after playback stops, for [REACHABLE_WINDOW_MS]. */
    private var inReachableWindow = false

    override fun onCreate() {
        super.onCreate()
        val audioManager = getSystemService(AudioManager::class.java)
        focus = AudioFocusController(audioManager, isPlaying = { runtime?.player?.isPlaying == true }, act = ::onFocus)
        usb = UsbBitPerfect(audioManager)
        noisy = NoisyReceiver(this) { runtime?.player?.pause() }
        routes = RouteWatcher(this, ::onRoute)
        player = KoanPlayer(
            Looper.getMainLooper(),
            controls = { controls },
            snapshot = { runtime?.let { PlayerSnapshot.of(it.mirror) } ?: PlayerSnapshot.Empty },
        )
        session = MediaSession.Builder(this, player)
            .setId("koan")
            .setSessionActivity(
                PendingIntent.getActivity(
                    this, 0, Intent(this, MainActivity::class.java),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                ),
            )
            .build()
        addSession(session!!)

        KoanProcess.ensureStarted(applicationContext)
        main.launch {
            KoanProcess.runtime.filterNotNull().collect(::attach)
        }
        routes.start()
    }

    /** The engine is up: hand the session its controls and follow the mirror with them. */
    private fun attach(runtime: KoanRuntime) {
        this.runtime = runtime
        controls = FocusedControls(runtime.player, focus)
        val mirror = runtime.mirror
        // Media3 reads the state again whenever one of these moves. None of
        // them is the position, which it derives from the anchor itself.
        main.launch {
            merge(
                mirror.playback.map { },
                mirror.playhead.map { },
                mirror.seekableMs.map { },
                mirror.queue.map { it.items.size }.distinctUntilChanged().map { },
            ).collect { player.refresh() }
        }
        main.launch {
            mirror.playhead.map { it.playing }.distinctUntilChanged().collect(::onPlayingChanged)
        }
        main.launch {
            // A new source rate is the cue to put a USB mixer at it, before the
            // engine opens the stream there, and then to read what was granted.
            mirror.playback.map { it.format?.sampleRate?.toInt() ?: 0 }.distinctUntilChanged()
                .collect { rate -> onSourceRate(rate) }
        }
        player.refresh()
    }

    private fun onPlayingChanged(playing: Boolean) {
        focus.onPlayingChanged(playing)
        noisy.setActive(playing)
        stopTimer?.cancel()
        if (playing) {
            inReachableWindow = false
        } else {
            // Paused or stopped: stay in the foreground, reachable by the
            // server's link, for a window, then let the service (and the link
            // with it) go. A silent loop, as on iOS, is not needed on Android.
            inReachableWindow = true
            stopTimer = main.launch {
                delay(REACHABLE_WINDOW_MS)
                inReachableWindow = false
                if (runtime?.player?.isPlaying != true) stopSelf()
            }
        }
    }

    private fun onFocus(action: FocusAction) {
        val model = runtime?.player ?: return
        when (action) {
            FocusAction.Pause, FocusAction.PauseUntilGain -> model.pause()
            FocusAction.Resume -> model.resume()
            // Ducking is the system's: see AudioFocusController.
            FocusAction.Duck, FocusAction.Unduck, FocusAction.None -> Unit
        }
    }

    private fun onRoute(route: OutputRoute, device: android.media.AudioDeviceInfo?) {
        routeDevice = device
        val rate = runtime?.mirror?.playback?.value?.format?.sampleRate?.toInt() ?: 0
        val changed = usb.prepare(route, device, rate)
        KoanProcess.setRoute(route, usb.activeRate ?: 0)
        if (changed) KoanProcess.refreshBitPerfect(usb.activeRate != null)
    }

    private fun onSourceRate(rate: Int) {
        val route = routes.current() ?: return
        if (rate <= 0) return
        // Mixer first, then the engine hears of the route again and reopens
        // its stream at the new rate on it.
        if (usb.prepare(route, routeDevice, rate)) {
            KoanProcess.setRoute(route, usb.activeRate ?: 0)
        } else {
            KoanProcess.refreshBitPerfect(usb.activeRate != null)
        }
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    override fun onUpdateNotification(session: MediaSession, startInForegroundRequired: Boolean) {
        super.onUpdateNotification(session, startInForegroundRequired || inReachableWindow)
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Swiped from Recents: music that is playing carries on; anything else ends.
        val playing = session?.player?.let { it.playWhenReady && it.playbackState != Player.STATE_IDLE } == true
        if (!playing) stopSelf()
    }

    override fun onDestroy() {
        noisy.setActive(false)
        routes.stop()
        usb.release()
        if (runtime?.player?.isPlaying == true) runtime?.player?.pause()
        // The engine outlives this service (see KoanProcess); what leaves
        // with it is the saved position.
        KoanProcess.saveAndRelease()
        session?.release()
        session = null
        main.cancel()
        super.onDestroy()
    }

    companion object {
        /** How long a stopped service stays in the foreground and reachable. */
        const val REACHABLE_WINDOW_MS = 10 * 60 * 1000L
    }
}

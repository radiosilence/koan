package cc.blit.koan.playback

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import androidx.annotation.OptIn
import androidx.core.content.ContextCompat
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.CacheBitmapLoader
import androidx.media3.session.CommandButton
import androidx.media3.session.DefaultMediaNotificationProvider
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import cc.blit.koan.KoanApp
import cc.blit.koan.MainActivity
import cc.blit.koan.R
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ffi.RepeatMode
import cc.blit.koan.ui.nowplaying.nextRepeat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/**
 * Keeps playback alive in the background and shows it to the system: the
 * media notification, the lock screen, headsets and cars.
 *
 * Media3 makes this a foreground service while `KoanPlayer` reports playing.
 * The service also does what an output owned outside Media3 must do for
 * itself: hold audio focus while playing, pause when headphones are unplugged,
 * and rebuild the engine's output when the route changes, since an AAudio
 * stream does not follow the device it was opened on to the next one.
 */
@OptIn(UnstableApi::class)
class PlaybackService : MediaSessionService() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private lateinit var session: MediaSession
    private lateinit var audio: AudioManager
    private val engine get() = (application as KoanApp).mirror

    private var focus: AudioFocusRequest? = null
    /** Paused for a transient loss of focus, so to resume when it comes back. */
    private var resumeOnGain = false

    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            // `focusCommand` drops it while another device is controlled: the
            // phone's headphones say nothing about that device.
            if (intent.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) focusCommand { pause() }
        }
    }

    private val routes = object : AudioDeviceCallback() {
        // Registering reports every device present; only changes after that matter.
        private var settled = false

        override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) {
            if (settled) restart() else settled = true
        }

        override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) = restart()

        private fun restart() = engine.launch { restartOutput() }
    }

    @OptIn(UnstableApi::class)
    override fun onCreate() {
        super.onCreate()
        audio = getSystemService(AudioManager::class.java)
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        // The ensō, white on clear: the status bar draws a small icon by its alpha alone.
        setMediaNotificationProvider(
            DefaultMediaNotificationProvider.Builder(this).build().apply { setSmallIcon(R.drawable.ic_notification) },
        )
        session = MediaSession.Builder(this, KoanPlayer(engine, scope, ArtProvider.authority(this)))
            .setSessionActivity(open)
            .setBitmapLoader(CacheBitmapLoader(ArtBitmapLoader(this, scope)))
            .build()
        ContextCompat.registerReceiver(
            this,
            noisy,
            IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        audio.registerAudioDeviceCallback(routes, null)
        // Focus is for sound this phone makes. While another device is
        // controlled, what plays is that device's, and taking focus for it
        // would silence the phone's other apps for nothing.
        scope.launch {
            combine(engine.nowPlaying, engine.devices) { now, devices ->
                devices.target == null && now?.state == PlayState.PLAYING
            }
                .distinctUntilChanged()
                .collect { playing -> if (playing) holdFocus() else if (!resumeOnGain) releaseFocus() }
        }
        // A resume owed to a transient loss here is not owed to the device
        // control moves to: let the focus go with it.
        scope.launch {
            engine.devices
                .map { it.target != null }
                .distinctUntilChanged()
                .collect { controlling -> if (controlling) releaseFocus() }
        }
        scope.launch {
            engine.nowPlaying
                .map { (it?.shuffle == true) to (it?.repeatMode ?: RepeatMode.OFF) }
                .distinctUntilChanged()
                .collect { (shuffle, repeat) -> session.setMediaButtonPreferences(modeButtons(shuffle, repeat)) }
        }
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo) = session

    /** Stop with the app's task when nothing is playing, as Media3 recommends. */
    override fun onTaskRemoved(rootIntent: Intent?) {
        // Swiped away with nothing playing, the process may go soon after.
        engine.flushSettings()
        if (!session.player.playWhenReady) stopSelf()
    }

    override fun onDestroy() {
        audio.unregisterAudioDeviceCallback(routes)
        unregisterReceiver(noisy)
        releaseFocus()
        session.player.release()
        session.release()
        scope.cancel()
        super.onDestroy()
    }

    /**
     * Shuffle and repeat beside the transport in the notification and on the
     * lock screen, each showing its state and setting the next one.
     */
    private fun modeButtons(shuffle: Boolean, repeat: RepeatMode) = listOf(
        CommandButton.Builder(if (shuffle) CommandButton.ICON_SHUFFLE_ON else CommandButton.ICON_SHUFFLE_OFF)
            .setDisplayName(if (shuffle) "Shuffle off" else "Shuffle")
            .setPlayerCommand(Player.COMMAND_SET_SHUFFLE_MODE, !shuffle)
            .setSlots(CommandButton.SLOT_OVERFLOW)
            .build(),
        CommandButton.Builder(
            when (repeat) {
                RepeatMode.OFF -> CommandButton.ICON_REPEAT_OFF
                RepeatMode.QUEUE -> CommandButton.ICON_REPEAT_ALL
                RepeatMode.ONE -> CommandButton.ICON_REPEAT_ONE
            },
        )
            .setDisplayName(
                when (repeat) {
                    RepeatMode.OFF -> "Repeat the queue"
                    RepeatMode.QUEUE -> "Repeat this track"
                    RepeatMode.ONE -> "Repeat off"
                },
            )
            .setPlayerCommand(
                Player.COMMAND_SET_REPEAT_MODE,
                mediaRepeat(nextRepeat(repeat)),
            )
            .setSlots(CommandButton.SLOT_OVERFLOW)
            .build(),
    )

    private fun holdFocus() {
        resumeOnGain = false
        if (focus != null) return
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            // Ducking is left to the system: koan's output has no volume of its own to lower.
            .setWillPauseWhenDucked(false)
            .setOnAudioFocusChangeListener(::onFocusChange)
            .build()
        if (audio.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
            focus = request
        } else {
            focusCommand { pause() }
        }
    }

    /**
     * What the phone's own audio asks of the player: a call, a notification,
     * headphones pulled out. None of it is about a device this phone
     * controls, so none of it is sent there, nor made to play here instead.
     */
    private fun focusCommand(block: suspend KoanEngine.() -> Unit) {
        if (engine.devices.value.target != null) return
        engine.launch { runCatching { block() } }
    }

    private fun releaseFocus() {
        focus?.let(audio::abandonAudioFocusRequest)
        focus = null
        resumeOnGain = false
    }

    private fun onFocusChange(change: Int) {
        when (change) {
            AudioManager.AUDIOFOCUS_LOSS -> {
                releaseFocus()
                focusCommand { pause() }
            }
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                resumeOnGain = engine.devices.value.target == null && engine.nowPlaying.value?.state == PlayState.PLAYING
                focusCommand { pause() }
            }
            AudioManager.AUDIOFOCUS_GAIN -> if (resumeOnGain) {
                resumeOnGain = false
                focusCommand { resume() }
            }
        }
    }
}

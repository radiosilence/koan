package cc.blit.koan.service

import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Handler
import android.os.Looper

/**
 * Audio focus for music. Asks before playing, gives it back on pause, and
 * turns the system's focus callbacks into [FocusAction]s through
 * [AudioFocusPolicy]. Main thread only.
 *
 * Ducking is the system's, not ours: the request leaves
 * `willPauseWhenDucked` at its default, so on a transient loss that allows it
 * the framework lowers the stream itself, and the engine keeps decoding. The
 * engine has no gain of its own for this; whether the framework's ducking
 * reaches an AAudio stream on a given device is a thing only a device shows.
 */
class AudioFocusController(
    private val audioManager: AudioManager,
    private val isPlaying: () -> Boolean,
    private val act: (FocusAction) -> Unit,
) {
    private val policy = AudioFocusPolicy()
    private var request: AudioFocusRequest? = null
    private var held = false

    private val listener = AudioManager.OnAudioFocusChangeListener { change ->
        act(policy.onFocusChange(change, isPlaying()))
    }

    private val attributes = AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_MEDIA)
        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
        .build()

    /** True when playing may start now. */
    fun acquire(): Boolean {
        val focusRequest = request ?: AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(attributes)
            .setOnAudioFocusChangeListener(listener, Handler(Looper.getMainLooper()))
            .build()
            .also { request = it }
        held = true
        return audioManager.requestAudioFocus(focusRequest) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
    }

    /** The user paused or stopped: nothing the focus does may resume the music by itself. */
    fun userPaused() = policy.userPaused()

    /**
     * Playback started or stopped, whoever asked for it. Stopping lets focus
     * go unless it was this that paused the music, which waits for the gain.
     */
    fun onPlayingChanged(playing: Boolean) {
        if (playing) {
            if (!held) acquire()
        } else if (!policy.pausedByFocus) {
            held = false
            request?.let { audioManager.abandonAudioFocusRequest(it) }
        }
    }
}

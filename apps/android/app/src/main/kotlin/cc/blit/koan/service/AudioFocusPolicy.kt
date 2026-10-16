package cc.blit.koan.service

import android.media.AudioManager

/** What the player must do about a change of audio focus. */
enum class FocusAction {
    /** Nothing to do. */
    None,

    /** Pause, and do not come back by ourselves: another app took focus for good. */
    Pause,

    /** Pause, and resume when focus returns: a call or an alert took it for a while. */
    PauseUntilGain,

    /** Keep playing, quieter. */
    Duck,

    /** Back to full level. */
    Unduck,

    /** Focus is back after a transient loss that paused us: resume. */
    Resume,
}

/**
 * The decisions behind audio focus, free of `AudioManager` so a JVM test runs
 * them: pause on loss, duck on transient loss that allows it, and come back
 * only from what was transient and only if it was this that paused the music.
 *
 * Not thread-safe: called from the main thread, as the focus listener is.
 */
class AudioFocusPolicy {
    /** This policy paused the music and will resume it on gain. */
    var pausedByFocus = false
        private set
    var ducked = false
        private set

    /** The user (or a media button) paused or stopped: no focus change may resume it. */
    fun userPaused() {
        pausedByFocus = false
    }

    fun onFocusChange(change: Int, playing: Boolean): FocusAction = when (change) {
        AudioManager.AUDIOFOCUS_LOSS -> {
            pausedByFocus = false
            ducked = false
            if (playing) FocusAction.Pause else FocusAction.None
        }
        AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
            ducked = false
            if (playing) {
                pausedByFocus = true
                FocusAction.PauseUntilGain
            } else {
                FocusAction.None
            }
        }
        AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> {
            if (playing && !ducked) {
                ducked = true
                FocusAction.Duck
            } else {
                FocusAction.None
            }
        }
        AudioManager.AUDIOFOCUS_GAIN -> when {
            pausedByFocus -> {
                pausedByFocus = false
                ducked = false
                FocusAction.Resume
            }
            ducked -> {
                ducked = false
                FocusAction.Unduck
            }
            else -> FocusAction.None
        }
        else -> FocusAction.None
    }
}

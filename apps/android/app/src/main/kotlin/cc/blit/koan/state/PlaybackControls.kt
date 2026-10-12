package cc.blit.koan.state

import uniffi.koan_ffi.RepeatMode

/**
 * What a media session can ask of the player. [PlayerModel] is the one real
 * implementation; the service wraps it to take audio focus around play and
 * give it back on pause, and a test uses a recorder.
 *
 * Every call returns at once: the work runs on the engine's dispatcher.
 */
interface PlaybackControls {
    fun resume()
    fun pause()
    fun stop()
    fun next()
    fun previous()
    fun seekTo(requestedMs: Long)
    fun setShuffle(on: Boolean)
    fun setRepeat(mode: RepeatMode)
}

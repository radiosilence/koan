package cc.blit.koan.service

import cc.blit.koan.state.PlaybackControls
import uniffi.koan_ffi.RepeatMode

/**
 * [PlaybackControls] with audio focus around them: asked for before the
 * engine is told to play, and a pause the user asked for cannot be undone by a
 * focus gain. Runs on Main, where the media session calls.
 */
class FocusedControls(
    private val inner: PlaybackControls,
    private val focus: AudioFocusController,
) : PlaybackControls {
    override fun resume() {
        // Not granted (a call in progress): do not play over whoever has it.
        if (focus.acquire()) inner.resume()
    }

    override fun pause() {
        focus.userPaused()
        inner.pause()
    }

    override fun stop() {
        focus.userPaused()
        inner.stop()
    }

    override fun next() = inner.next()
    override fun previous() = inner.previous()
    override fun seekTo(requestedMs: Long) = inner.seekTo(requestedMs)
    override fun setShuffle(on: Boolean) = inner.setShuffle(on)
    override fun setRepeat(mode: RepeatMode) = inner.setRepeat(mode)
}

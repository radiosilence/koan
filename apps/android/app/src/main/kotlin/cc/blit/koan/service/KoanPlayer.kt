package cc.blit.koan.service

import android.os.Looper
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.common.SimpleBasePlayer
import cc.blit.koan.state.EngineMirror
import cc.blit.koan.state.PlaybackControls
import cc.blit.koan.state.Playhead
import cc.blit.koan.state.QueueState
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import uniffi.koan_ffi.NowPlaying
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.RepeatMode

/** Everything [KoanPlayerState] reads, taken at one moment from the mirror. */
data class PlayerSnapshot(
    val playback: NowPlaying,
    val playhead: Playhead,
    val seekableMs: Long,
    val queueSize: Int,
) {
    companion object {
        fun of(mirror: EngineMirror): PlayerSnapshot = PlayerSnapshot(
            mirror.playback.value, mirror.playhead.value, mirror.seekableMs.value,
            mirror.queue.value.size(),
        )

        val Empty = PlayerSnapshot(EngineMirror.idle, Playhead.Stopped, 0, 0)
    }
}

private fun QueueState.size() = items.size

/**
 * The mirror's state, as Media3's. Pure: no looper, no session, so a JVM test
 * covers the mapping.
 *
 * **The timeline is the current track only.** A media session needs the
 * current item to draw a notification and a lock screen, and the buttons to
 * ask for the next one; it does not need five hundred rows, which would be
 * rebuilt on every state change. The queue is the engine's, and next and
 * previous are commands rather than index arithmetic over a timeline, so the
 * availability of those buttons is stated rather than derived. Android Auto
 * browsing, which does want the tree, comes with its own library service.
 */
object KoanPlayerState {
    fun map(snapshot: PlayerSnapshot): SimpleBasePlayer.State {
        val playback = snapshot.playback
        val entry = playback.entry
        val hasItem = playback.queueItemId != null && entry != null
        val builder = SimpleBasePlayer.State.Builder()
            .setAvailableCommands(commands(snapshot))
            .setShuffleModeEnabled(playback.shuffle)
            .setRepeatMode(repeat(playback.repeatMode))

        if (!hasItem) {
            return builder
                .setPlaybackState(Player.STATE_IDLE)
                .setPlayWhenReady(false, Player.PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST)
                .build()
        }

        val durationMs = playback.durationMs.toLong()
        val metadata = MediaMetadata.Builder()
            .setTitle(entry.title)
            .setArtist(entry.artist)
            .setAlbumArtist(entry.albumArtist.ifEmpty { null })
            .setAlbumTitle(entry.album.ifEmpty { null })
            .setTrackNumber(entry.trackNumber?.toInt())
            .setDiscNumber(entry.disc?.toInt())
            .setIsPlayable(true)
            .setIsBrowsable(false)
            .build()
        val item = SimpleBasePlayer.MediaItemData.Builder(playback.queueItemId!!)
            .setMediaItem(MediaItem.Builder().setMediaId(playback.queueItemId!!).setMediaMetadata(metadata).build())
            .setMediaMetadata(metadata)
            .setDurationUs(if (durationMs > 0) durationMs * 1000 else C.TIME_UNSET)
            .setIsSeekable(snapshot.seekableMs > 0)
            .setIsDynamic(false)
            .build()

        val playing = playback.state == PlayState.PLAYING
        val buffering = playback.waiting && playback.state == PlayState.STOPPED
        // The position is derived from the anchor when Media3 asks, not pushed
        // ten times a second: a playhead advancing at one second per second is
        // the one thing a client can work out for itself.
        val playhead = snapshot.playhead
        val position = SimpleBasePlayer.PositionSupplier { playhead.at(withinMs = durationMs) }
        return builder
            .setPlaylist(listOf(item))
            .setCurrentMediaItemIndex(0)
            .setPlaybackState(if (buffering) Player.STATE_BUFFERING else Player.STATE_READY)
            .setPlayWhenReady(playing || buffering, Player.PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST)
            .setContentPositionMs(position)
            .build()
    }

    fun repeat(mode: RepeatMode): Int = when (mode) {
        RepeatMode.OFF -> Player.REPEAT_MODE_OFF
        RepeatMode.QUEUE -> Player.REPEAT_MODE_ALL
        RepeatMode.ONE -> Player.REPEAT_MODE_ONE
    }

    fun repeat(mode: Int): RepeatMode = when (mode) {
        Player.REPEAT_MODE_ALL -> RepeatMode.QUEUE
        Player.REPEAT_MODE_ONE -> RepeatMode.ONE
        else -> RepeatMode.OFF
    }

    /**
     * Which commands are offered, as plain ints. Media3's `Commands` is built
     * on `SparseBooleanArray`, which is a no-op stub in a JVM test, so the
     * decision is made here and the test reads it here.
     */
    fun commandIds(snapshot: PlayerSnapshot): Set<Int> {
        val hasItem = snapshot.playback.queueItemId != null && snapshot.playback.entry != null
        val ids = mutableSetOf(
            Player.COMMAND_GET_CURRENT_MEDIA_ITEM,
            Player.COMMAND_GET_TIMELINE,
            Player.COMMAND_GET_METADATA,
            Player.COMMAND_PREPARE,
            Player.COMMAND_SET_SHUFFLE_MODE,
            Player.COMMAND_SET_REPEAT_MODE,
        )
        if (snapshot.queueSize > 0 || hasItem) {
            ids += listOf(
                Player.COMMAND_PLAY_PAUSE,
                Player.COMMAND_STOP,
                Player.COMMAND_SEEK_TO_NEXT,
                Player.COMMAND_SEEK_TO_NEXT_MEDIA_ITEM,
                Player.COMMAND_SEEK_TO_PREVIOUS,
                Player.COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM,
            )
        }
        if (hasItem && snapshot.seekableMs > 0) {
            ids += listOf(
                Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM,
                Player.COMMAND_SEEK_BACK,
                Player.COMMAND_SEEK_FORWARD,
            )
        }
        return ids
    }

    private fun commands(snapshot: PlayerSnapshot): Player.Commands =
        Player.Commands.Builder().addAll(*commandIds(snapshot).toIntArray()).build()
}

/**
 * The session's player: reports the mirror's state and forwards what the
 * session is asked to the engine through [PlaybackControls]. It owns nothing;
 * playback belongs to the engine and the engine to the service.
 *
 * Main thread, as Media3 requires: [refresh] is called there when a mirror
 * flow moves, and everything else here only reads a snapshot or issues a
 * command that returns at once.
 */
class KoanPlayer(
    looper: Looper,
    private val controls: () -> PlaybackControls?,
    private val snapshot: () -> PlayerSnapshot,
) : SimpleBasePlayer(looper) {

    override fun getState(): State = KoanPlayerState.map(snapshot())

    /** The mirror moved: ask Media3 to read [getState] again. */
    fun refresh() = invalidateState()

    private inline fun command(body: PlaybackControls.() -> Unit): ListenableFuture<*> {
        controls()?.body()
        return Futures.immediateVoidFuture()
    }

    override fun handleSetPlayWhenReady(playWhenReady: Boolean): ListenableFuture<*> =
        command { if (playWhenReady) resume() else pause() }

    override fun handlePrepare(): ListenableFuture<*> = Futures.immediateVoidFuture()

    override fun handleStop(): ListenableFuture<*> = command { stop() }

    override fun handleSetShuffleModeEnabled(shuffleModeEnabled: Boolean): ListenableFuture<*> =
        command { setShuffle(shuffleModeEnabled) }

    override fun handleSetRepeatMode(repeatMode: Int): ListenableFuture<*> =
        command { setRepeat(KoanPlayerState.repeat(repeatMode)) }

    override fun handleSeek(mediaItemIndex: Int, positionMs: Long, seekCommand: Int): ListenableFuture<*> =
        command {
            when (seekCommand) {
                COMMAND_SEEK_TO_NEXT, COMMAND_SEEK_TO_NEXT_MEDIA_ITEM -> next()
                COMMAND_SEEK_TO_PREVIOUS, COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM -> previous()
                else -> if (positionMs != C.TIME_UNSET) seekTo(positionMs)
            }
        }

    /** Release is the service's business: the player is the session's, not the engine's. */
    override fun handleRelease(): ListenableFuture<*> = Futures.immediateVoidFuture()
}

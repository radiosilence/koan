package cc.blit.koan.playback

import android.os.Looper
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.common.SimpleBasePlayer
import androidx.media3.common.util.UnstableApi
import cc.blit.koan.engine.Engine
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ffi.QueueItem
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.SettableFuture
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/**
 * The engine as a Media3 `Player`, so a `MediaSession` can show it and drive it.
 *
 * Nothing plays through Media3: the engine owns decoding and output. This
 * player reports the engine's queue and playhead, and turns the session's
 * commands (the notification, headset buttons, Bluetooth, Android Auto) into
 * engine calls. It lives on the main looper, as Media3 requires, and `scope`
 * must run there too.
 */
@OptIn(UnstableApi::class)
class KoanPlayer(
    private val engine: Engine,
    private val scope: CoroutineScope,
) : SimpleBasePlayer(Looper.getMainLooper()) {
    private var mapped: Pair<List<QueueItem>, List<MediaItemData>> = emptyList<QueueItem>() to emptyList()

    init {
        scope.launch {
            combine(engine.nowPlaying, engine.queue, engine.playhead) { _, _, _ -> }
                .collect { invalidateState() }
        }
    }

    override fun getState(): State {
        val now = engine.nowPlaying.value
        val queue = engine.queue.value
        val playhead = engine.playhead.value
        val items = playlist(queue)
        val current = queue.indexOfFirst { it.queueItemId == now?.queueItemId }
        val state = when {
            items.isEmpty() -> STATE_IDLE
            current < 0 -> STATE_ENDED
            now?.waiting == true -> STATE_BUFFERING
            else -> STATE_READY
        }
        return State.Builder()
            .setAvailableCommands(COMMANDS)
            .setPlaylist(items)
            .setCurrentMediaItemIndex(current.coerceAtLeast(0))
            .setPlaybackState(state)
            .setPlayWhenReady(now?.state == PlayState.PLAYING, PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST)
            .setContentPositionMs(
                PositionSupplier.getExtrapolating(playhead.positionMs, if (playhead.playing) 1f else 0f),
            )
            .build()
    }

    /** The queue as Media3 items, rebuilt only when the queue itself changed. */
    private fun playlist(queue: List<QueueItem>): List<MediaItemData> {
        if (mapped.first !== queue) mapped = queue to queue.map(::itemData)
        return mapped.second
    }

    private fun itemData(item: QueueItem) = MediaItemData.Builder(item.queueItemId)
        .setMediaItem(
            MediaItem.Builder()
                .setMediaId(item.queueItemId)
                .setMediaMetadata(
                    MediaMetadata.Builder()
                        .setTitle(item.title)
                        .setArtist(item.artist)
                        .setAlbumTitle(item.album)
                        .setAlbumArtist(item.albumArtist)
                        .build(),
                )
                .build(),
        )
        .setDurationUs(item.durationMs?.let { it.toLong() * 1000 } ?: C.TIME_UNSET)
        .build()

    override fun handleSetPlayWhenReady(playWhenReady: Boolean) =
        call { if (playWhenReady) resume() else pause() }

    override fun handleSeek(mediaItemIndex: Int, positionMs: Long, seekCommand: Int) = call {
        when (seekCommand) {
            COMMAND_SEEK_TO_NEXT, COMMAND_SEEK_TO_NEXT_MEDIA_ITEM -> next()
            COMMAND_SEEK_TO_PREVIOUS, COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM -> previous()
            COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM -> seek(positionMs.coerceAtLeast(0).toULong())
            else -> engine.queue.value.getOrNull(mediaItemIndex)?.let { play(it.queueItemId) }
        }
    }

    override fun handleStop() = call { pause() }

    override fun handleRelease(): ListenableFuture<*> = Futures.immediateVoidFuture()

    /** Run an engine call, settling the future once the engine has taken it. */
    private fun call(block: suspend KoanEngine.() -> Unit): ListenableFuture<*> {
        val done = SettableFuture.create<Unit>()
        scope.launch {
            runCatching { engine.get().block() }
            done.set(Unit)
        }
        return done
    }

    private companion object {
        val COMMANDS: Player.Commands = Player.Commands.Builder()
            .addAll(
                COMMAND_PLAY_PAUSE,
                COMMAND_STOP,
                COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM,
                COMMAND_SEEK_TO_NEXT,
                COMMAND_SEEK_TO_NEXT_MEDIA_ITEM,
                COMMAND_SEEK_TO_PREVIOUS,
                COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM,
                COMMAND_SEEK_TO_MEDIA_ITEM,
                COMMAND_GET_CURRENT_MEDIA_ITEM,
                COMMAND_GET_TIMELINE,
                COMMAND_GET_METADATA,
                COMMAND_RELEASE,
            )
            .build()
    }
}

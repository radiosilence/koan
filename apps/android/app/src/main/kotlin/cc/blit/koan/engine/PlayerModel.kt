package cc.blit.koan.engine

import cc.blit.koan.ffi.KoanEngine
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.withTimeoutOrNull

/**
 * What the app does to the player. State is read from `EngineMirror`; this
 * only sends. Each call returns at once and reaches the engine in the order
 * made: koan-ffi sends transport and queue commands down one lane.
 */
class PlayerModel(private val mirror: EngineMirror) {
    fun play(queueItemId: String) = mirror.launch { play(queueItemId) }

    /** Replace the queue with `trackIds` and play from `startAt`, in order or shuffled. */
    fun playTracks(trackIds: List<Long>, startAt: Int = 0, shuffled: Boolean = false) =
        mirror.launch { replaceQueue(trackIds, startAt.toUInt(), shuffled) }

    fun enqueue(trackIds: List<Long>) = mirror.launch { addToQueue(trackIds) }

    fun playNext(trackIds: List<Long>) {
        val current = mirror.nowPlaying.value?.queueItemId ?: return enqueue(trackIds)
        mirror.launch { insertAfter(trackIds, current) }
    }

    fun remove(queueItemIds: List<String>) = mirror.launch { removeFromQueue(queueItemIds) }

    fun togglePlayPause() = mirror.launch { togglePlayPause() }

    fun pause() = mirror.launch { pause() }

    fun resume() = mirror.launch { resume() }

    fun next() = mirror.launch { next() }

    fun previous() = mirror.launch { previous() }

    fun seek(positionMs: Long) = mirror.launch { seek(positionMs.coerceAtLeast(0).toULong()) }

    fun setShuffle(on: Boolean) = mirror.launch { setShuffle(on) }

    /** The queue version last written whole, so the queue is rewritten only when it is what changed. */
    @Volatile private var savedQueueVersion = -1L

    /**
     * Bring back the last session's queue without playing it, then keep it
     * saved: every second while the music plays, so a killed process costs a
     * second, and at each pause or stop. A queue the size of a library is
     * only rewritten when it changed; otherwise only the position is.
     */
    fun followSession() = mirror.launch {
        val restored = runCatching { restoreSession() }.getOrDefault(0u)
        // The player thread applies the restored rows; saving before they
        // arrive would write the empty queue over the one just read.
        if (restored > 0u) withTimeoutOrNull(500) { mirror.queue.first { it.items.size >= restored.toInt() } }
        savedQueueVersion = mirror.queue.value.version
        mirror.playhead.map { it.playing }.distinctUntilChanged().drop(1).collectLatest { playing ->
            if (playing) {
                while (true) {
                    delay(1_000)
                    save()
                }
            } else {
                save()
            }
        }
    }

    /** The app is leaving the screen: the session here, and the queue on the server when this device keeps it there. */
    fun saveOnLeaving() = mirror.launch {
        runCatching { saveSession() }.onSuccess { savedQueueVersion = mirror.queue.value.version }
        saveServerQueueNow()
    }

    private suspend fun KoanEngine.save() {
        val version = mirror.queue.value.version
        runCatching {
            if (version != savedQueueVersion) {
                saveSession()
                savedQueueVersion = version
            } else if (mirror.nowPlaying.value != null) {
                savePosition()
            }
        }
    }
}

package cc.blit.koan.engine

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

    /** Rebuild the output where playback is: after the route or an output setting changed. */
    fun restartOutput() = mirror.launch { restartOutput() }
}

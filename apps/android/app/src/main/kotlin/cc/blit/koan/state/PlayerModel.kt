package cc.blit.koan.state

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.RepeatMode
import uniffi.koan_ffi.SleepTimer

/** The record and the artist behind what is playing, for the transport to link to. */
data class Place(val trackId: Long, val albumId: Long?, val artistId: Long?)

/**
 * What the app does to the player, and the little it knows that the engine
 * does not. The Kotlin port of `PlayerModel.swift`; its doc comments are the
 * specification.
 *
 * Not a copy of engine state — that is [EngineMirror], and everything read
 * here reads through it. What lives here is the other direction: commands,
 * the busy count and the error banner they need, and the handful of things
 * that are local because the engine has no opinion about them, such as where
 * a thumb is being dragged to.
 *
 * **Threading.** Every engine call is launched on [scope], whose dispatcher
 * is the engine's (see `EngineDispatcher`), never Main: a UI thread that
 * waits on a crossing drops frames. The commands return at once. The methods
 * are callable from any thread; the state they keep is flows and a lock.
 *
 * Left for the pages that need them: the devices and renderer menus, the
 * pasteboard and file drops, Edit-menu selection (a phone has no Edit
 * menu), and `ActivityModel`'s task registry.
 */
class PlayerModel(
    private val engine: KoanEngineInterface,
    val mirror: EngineMirror,
    private val scope: CoroutineScope,
    private val nanoClock: () -> Long = System::nanoTime,
) : PlaybackControls {
    private val _scrubbing = MutableStateFlow<Double?>(null)
    private val _lastError = MutableStateFlow<String?>(null)
    private val _lastNotice = MutableStateFlow<String?>(null)
    private val _pendingMutations = MutableStateFlow(0)
    private val _resolving = MutableStateFlow<String?>(null)
    private val _place = MutableStateFlow<Place?>(null)

    /**
     * Set while the user drags the seek head, so the engine's own position
     * doesn't yank the thumb back mid-gesture. A 0–1 fraction.
     */
    val scrubbing: StateFlow<Double?> = _scrubbing.asStateFlow()

    val lastError: StateFlow<String?> = _lastError.asStateFlow()

    /** Something the app declined to do, and why. Not a failure: the state it describes resolves on its own. */
    val lastNotice: StateFlow<String?> = _lastNotice.asStateFlow()

    /**
     * Queue mutations in flight. Adding a large selection takes a moment, and
     * silence while it happens reads as nothing having happened.
     */
    val pendingMutations: StateFlow<Int> = _pendingMutations.asStateFlow()

    /**
     * What a play still finding its tracks is for, from the tap until the
     * engine holds the new queue: the transport names it, loading, at once,
     * rather than the paused track it is about to replace. Play and pause are
     * ignored meanwhile, so that track cannot be resumed under it.
     */
    val resolving: StateFlow<String?> = _resolving.asStateFlow()

    /**
     * The record and artist of what is playing, resolved once when the track
     * changes (it is a database read) and null until then. A `QueueItem`
     * carries names, not ids: it has to stand for things never in the library.
     */
    val place: StateFlow<Place?> = _place.asStateFlow()

    fun dismissError() { _lastError.value = null }
    fun dismissNotice() { _lastNotice.value = null }
    fun report(message: String) { _lastError.value = message }

    // MARK: - Following

    /**
     * Start following the mirror for what is not a screen: where a seek asked
     * to land, where what is playing lives, and the session autosave. Returns
     * the job holding them; cancel it to stop.
     */
    fun start(): Job = scope.launch {
        launch {
            mirror.playback.map { it.entry?.trackId }.distinctUntilChanged().collect(::resolvePlace)
        }
        launch {
            mirror.playhead.collect { settlePendingSeek(it.at(nanoClock())) }
        }
        launch {
            var saving: Job? = null
            mirror.playhead.map { it.playing }.distinctUntilChanged().collect { playing ->
                saving?.cancel()
                saving = if (playing) launch { keepSaving() } else null
                // A pause, a stop or a track change is a moment worth
                // remembering in its own right, and the last one before a
                // long silence.
                if (!playing) launch { save() }
            }
        }
    }

    private suspend fun resolvePlace(trackId: Long?) {
        if (trackId == null) {
            _place.value = null
            return
        }
        _place.value = null
        val track = try {
            engine.track(trackId)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            null
        }
        // The track moved on while we were asking, so whatever came back
        // belongs to something that is no longer playing.
        if (trackId != currentTrackId) return
        _place.value = Place(trackId, track?.albumId, track?.artistId)
    }

    // MARK: - What is playing
    //
    // Read straight through the mirror: one account of each of these and no
    // rule about when to refresh it.

    val isPlaying: Boolean get() = mirror.playback.value.state == PlayState.PLAYING
    val shuffle: Boolean get() = mirror.playback.value.shuffle
    val repeatMode: RepeatMode get() = mirror.playback.value.repeatMode
    val currentTrackId: Long? get() = mirror.playback.value.entry?.trackId
    val currentItemId: String? get() = mirror.playback.value.queueItemId
    val durationMs: Long get() = mirror.playback.value.durationMs.toLong()

    /**
     * Asked to play a track that has not arrived yet, which with nothing on
     * screen to say so reads as a tap that did nothing. A wait paused by hand
     * reads as paused, since it will open paused.
     */
    val isWaitingForTrack: Boolean
        get() = _resolving.value != null ||
            (mirror.playback.value.waiting && mirror.playback.value.state == PlayState.STOPPED)

    /**
     * 0–1 through the current track, as of now. Reflects the drag while
     * scrubbing. A value for a moment, not one that arrives: anything drawing
     * it continuously should animate toward the end of the track.
     */
    fun progress(nowNanos: Long = nanoClock()): Double {
        _scrubbing.value?.let { return it }
        val duration = durationMs
        if (duration <= 0) return 0.0
        return minOf(1.0, mirror.playhead.value.at(nowNanos, duration).toDouble() / duration)
    }

    /**
     * Whether the track can be seeked at all yet. False while a download is
     * playing that could not say what it is until the rest of it lands; it
     * becomes true on its own when the transfer finishes.
     */
    val canSeek: Boolean get() = mirror.seekableMs.value > 0

    /**
     * How much of the track can be reached, as a fraction: 1 for anything on
     * disk, short of it while a download is still arriving.
     */
    val seekable: Double
        get() {
            val seekableMs = mirror.seekableMs.value
            val duration = durationMs
            return if (duration <= 0 || seekableMs >= duration) 1.0 else seekableMs.toDouble() / duration
        }

    // MARK: - Transport

    fun togglePlayPause() {
        if (_resolving.value != null) return
        attempt { engine.togglePlayPause() }
    }

    override fun pause() = attempt { engine.pause() }

    override fun resume() {
        if (_resolving.value != null) return
        attempt { engine.resume() }
    }

    override fun next() = attempt { engine.next() }
    override fun previous() = attempt { engine.previous() }
    override fun stop() = attempt { engine.stop() }

    override fun setShuffle(on: Boolean) = attempt { engine.setShuffle(on) }
    override fun setRepeat(mode: RepeatMode) = attempt { engine.setRepeat(mode) }
    fun toggleShuffle() = setShuffle(!shuffle)

    /** Off, the queue, one, and round again — the one button's steps. */
    fun cycleRepeat() = setRepeat(
        when (repeatMode) {
            RepeatMode.OFF -> RepeatMode.QUEUE
            RepeatMode.QUEUE -> RepeatMode.ONE
            RepeatMode.ONE -> RepeatMode.OFF
        },
    )

    fun setSleepTimer(timer: SleepTimer) = attempt { engine.setSleepTimer(timer) }
    fun cancelSleepTimer() = attempt { engine.cancelSleepTimer() }

    fun play(itemId: String) = attempt { engine.play(itemId) }

    // MARK: - Seeking

    private val lock = Any()
    private var pendingSeekMs: Long? = null
    private var pendingSeekDeadline: Long? = null

    /**
     * Release the held position once the engine has caught up — or give up, so
     * a seek the engine rejected can't wedge the bar permanently.
     */
    private fun settlePendingSeek(positionMs: Long) = synchronized(lock) {
        val target = pendingSeekMs ?: return@synchronized
        val reached = kotlin.math.abs(positionMs - target) < SEEK_TOLERANCE_MS
        val expired = pendingSeekDeadline?.let { nanoClock() >= it } ?: true
        if (reached || expired) {
            pendingSeekMs = null
            pendingSeekDeadline = null
            _scrubbing.value = null
        }
    }

    /**
     * Commit a scrub. The position comes from the drag, not the engine, and
     * stops at what has been downloaded.
     */
    fun seek(fraction: Double) = seekTo((clamp(fraction) * durationMs).toLong())

    /**
     * Called as the thumb is dragged. Cancels any seek still settling, since
     * the user is now the authority on where the head is.
     */
    fun beginScrub(fraction: Double) {
        synchronized(lock) {
            pendingSeekMs = null
            pendingSeekDeadline = null
        }
        _scrubbing.value = clamp(fraction)
    }

    /** A drag position held inside the track and inside what has arrived. */
    private fun clamp(fraction: Double): Double = minOf(seekable, minOf(1.0, maxOf(0.0, fraction)))

    /**
     * Why the playhead did not move. Reaching for a position in a track that
     * has not arrived is a reasonable thing to try, and a bar that simply
     * ignores the attempt teaches nothing.
     */
    fun explainUnseekable() {
        val fetched = currentTrackId?.let { mirror.figures.value[it] }
        val so = fetched?.progress?.let { " — ${(it * 100).toInt()}% so far" } ?: ""
        _lastNotice.value = "Still downloading$so. This track can be seeked once it has finished."
    }

    /** Nudge by a number of seconds, clamped to the track. */
    fun seekBy(seconds: Int) {
        if (!canSeek) return explainUnseekable()
        val current = mirror.playhead.value.at(nanoClock())
        val target = maxOf(0L, current + seconds * 1000L)
        seekTo(minOf(target, mirror.seekableMs.value))
    }

    /**
     * Hold the requested position until the engine agrees with it. The seek
     * is asynchronous: it goes down a channel to the player thread, which
     * restarts decoding before the position moves. Clearing the local value
     * when the command is merely *sent* hands the bar back to the engine
     * during that gap, and the thumb snaps backwards before jumping forward.
     */
    override fun seekTo(requestedMs: Long) {
        if (!canSeek) return explainUnseekable()
        val ms = minOf(requestedMs, mirror.seekableMs.value)
        val deadline = nanoClock() + SEEK_PATIENCE_MS * 1_000_000L
        synchronized(lock) {
            pendingSeekMs = ms
            pendingSeekDeadline = deadline
        }
        if (durationMs > 0) _scrubbing.value = ms.toDouble() / durationMs.toDouble()
        attempt { engine.seek(ms.toULong()) }
        // The playhead may never move again to settle a rejected seek.
        scope.launch {
            delay(SEEK_PATIENCE_MS)
            val current = synchronized(lock) { pendingSeekDeadline }
            if (current == deadline) settlePendingSeek(mirror.playhead.value.at(nanoClock()))
        }
    }

    // MARK: - Queue

    /**
     * Replace the queue with what `resolve` finds, and play it. The transport
     * says [resolving] from now until the engine holds the new queue.
     */
    fun playNow(name: String, shuffled: Boolean = false, resolve: suspend () -> List<Long>): Job {
        _resolving.value = name
        _pendingMutations.value += 1
        return scope.launch {
            try {
                val ids = resolve()
                if (ids.isNotEmpty()) engine.replaceQueue(ids, 0u, shuffled)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _lastError.value = describe(e)
            } finally {
                _resolving.value = null
                _pendingMutations.value -= 1
            }
        }
    }

    /** Replace the queue with these tracks and play from `index`. */
    fun playNow(trackIds: List<Long>, startingAt: Int? = null) {
        if (trackIds.isEmpty()) return
        val (queued, start) = listing(trackIds, startingAt)
        mutate { engine.replaceQueue(queued, start.toUInt(), false) }
    }

    fun playNext(trackIds: List<Long>) {
        if (trackIds.isEmpty()) return
        val cursor = currentItemId ?: return enqueue(trackIds)
        mutate { engine.insertAfter(trackIds, cursor) }
    }

    fun enqueue(trackIds: List<Long>) {
        if (trackIds.isEmpty()) return
        mutate { engine.addToQueue(trackIds) }
    }

    fun remove(itemIds: List<String>) {
        if (itemIds.isEmpty()) return
        mutate { engine.removeFromQueue(itemIds) }
    }

    fun move(itemIds: List<String>, target: String, after: Boolean) =
        mutate { engine.moveInQueue(itemIds, target, after) }

    fun clearQueue() = attempt { engine.clearQueue() }
    fun undo(playlist: Long? = null) = attempt { engine.undo(playlist) }
    fun redo(playlist: Long? = null) = attempt { engine.redo(playlist) }

    // MARK: - Session

    private var savedQueueVersion = 0L

    suspend fun saveSession() {
        runCatching { engine.saveSession() }.onFailure { if (it is CancellationException) throw it }
        savedQueueVersion = mirror.queueRows.value.version
    }

    /** On leaving: the last save, and the server's copy of the queue now rather than at its next tick. */
    suspend fun saveOnLeaving() {
        saveSession()
        runCatching { engine.saveServerQueueNow() }.onFailure { if (it is CancellationException) throw it }
    }

    private suspend fun keepSaving() {
        while (true) {
            delay(1000)
            save()
        }
    }

    private suspend fun save() {
        val version = mirror.queueRows.value.version
        if (version != savedQueueVersion) {
            saveSession()
        } else if (mirror.playback.value.entry != null) {
            runCatching { engine.savePosition() }.onFailure { if (it is CancellationException) throw it }
        }
    }

    /** Put back the queue and position the last run left. Waits briefly for the mirror to hold it. */
    suspend fun restoreSession() {
        val restored = runCatching { engine.restoreSession() }.getOrNull() ?: return
        if (restored == 0u) return
        mirror.waitUntil(500) { mirror.queue.value.items.size >= restored.toInt() }
    }

    // MARK: - Errors

    private fun describe(e: Exception) = e.message ?: e.toString()

    /** Run a command off the caller's thread; a failure becomes the banner. */
    private fun attempt(body: suspend () -> Unit) {
        scope.launch {
            try {
                body()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _lastError.value = describe(e)
            }
        }
    }

    /** `attempt` for a queue edit: counts as busy while it runs. */
    private fun mutate(body: suspend () -> Unit) {
        _pendingMutations.value += 1
        attempt {
            try {
                body()
            } finally {
                _pendingMutations.value -= 1
            }
        }
    }

    companion object {
        /** Close enough to the target to count as having landed. */
        const val SEEK_TOLERANCE_MS = 750L

        /** How long a seek waits for the engine before handing the bar back. */
        const val SEEK_PATIENCE_MS = 2000L

        /**
         * How many tracks a listing is queued with: a play from the middle of
         * a very long listing sends the window after the track, not all of it.
         */
        const val LISTING_WINDOW = 500

        /** The ids to queue and the index to start at, for a play from `index` of a listing. */
        fun listing(ids: List<Long>, index: Int?): Pair<List<Long>, Int> {
            if (index == null) return ids to 0
            val start = if (index in ids.indices) index else 0
            if (ids.size <= LISTING_WINDOW) return ids to start
            return ids.subList(start, minOf(start + LISTING_WINDOW, ids.size)).toList() to 0
        }
    }
}

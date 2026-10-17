package cc.blit.koan.engine

import android.os.SystemClock
import cc.blit.koan.ffi.Appearance
import cc.blit.koan.ffi.ConnectionInfo
import cc.blit.koan.ffi.DeviceInfo
import cc.blit.koan.ffi.DeviceReach
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.NowPlaying
import cc.blit.koan.ffi.OutputsInfo
import cc.blit.koan.ffi.QueueItem
import cc.blit.koan.ffi.QueueLock
import cc.blit.koan.ffi.RendererInfo
import cc.blit.koan.ffi.RendererOutput
import cc.blit.koan.ffi.StateSlice
import cc.blit.koan.ffi.SyncProgress
import cc.blit.koan.ffi.Transfer
import cc.blit.koan.ffi.TransferFigure
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * Where the playhead was at `atMs` (`SystemClock.elapsedRealtime`), and
 * whether it is moving.
 *
 * An anchor, not a reading: the engine publishes it when it stops being true
 * (a seek, a pause, a new track, a stall), not on a clock. `positionAt` is
 * where it is now, for whatever draws a moving playhead at its own rate.
 */
data class Playhead(val positionMs: Long, val seekableMs: Long, val playing: Boolean, val atMs: Long) {
    /**
     * Where it is at `nowMs`, kept within `durationMs` when that is known: the
     * reckoning runs on past a track's end until the anchor for the next one
     * arrives, and a clock reading longer than the track is never right.
     */
    fun positionAt(nowMs: Long = SystemClock.elapsedRealtime(), durationMs: Long = 0): Long {
        val at = if (playing) positionMs + (nowMs - atMs) else positionMs
        return if (durationMs > 0) at.coerceAtMost(durationMs) else at
    }

    /** Whether this anchor only restates `previous`: the same motion, the same place give or take `toleranceMs`. */
    fun continues(previous: Playhead, toleranceMs: Long = 500) =
        playing == previous.playing && Math.abs(previous.positionAt(atMs) - positionMs) <= toleranceMs
}

/** The queue as the engine last sent it, with the latest row patch applied. */
data class QueueState(val items: List<QueueItem>, val version: Long)

data class Tasks(val scanning: Boolean, val syncing: Boolean)

/** `targetReach` is how the controlled device can be reached, from its connections; `null` while this phone is in view. */
data class Devices(val devices: List<DeviceInfo>, val target: String?, val targetReach: DeviceReach? = null)

data class Renderers(val renderers: List<RendererInfo>, val output: RendererOutput?)

/**
 * The engine's state as Kotlin sees it: one `StateFlow` per slice, the
 * counterpart of the Apple apps' `EngineMirror.swift`.
 *
 * One loop reads the engine's cursor (`KoanEngine.observe`) off the main
 * thread and sets each flow. The cursor waits until something changed, so an
 * idle engine costs nothing, and it never loses a change: a reader that falls
 * behind gets the latest whole value of each slice. A flow only emits when its
 * value differs, so a collector wakes for the slice it reads and no other.
 * Compose collects on the main thread with `collectAsStateWithLifecycle`.
 *
 * `library` and `history` are versions, not data: a page that lists the
 * library asks the engine again when the version moves.
 */
class EngineMirror(deviceName: String) {
    private val _startFailure = MutableStateFlow<String?>(null)
    /**
     * Why the engine did not open, when it did not: a library a newer build
     * migrated, a database that cannot be read. The app shows this and
     * nothing else; every call waiting on the engine ends quietly.
     */
    val startFailure: StateFlow<String?> = _startFailure.asStateFlow()

    /**
     * A call waiting on an engine that never opened ends here rather than
     * crashing the app; anything else that escapes goes to the thread's
     * handler, as it would have.
     */
    private val unopened = CoroutineExceptionHandler { _, e ->
        if (opened.isCompleted && opened.getCompletionExceptionOrNull() != null) return@CoroutineExceptionHandler
        Thread.currentThread().let { it.uncaughtExceptionHandler?.uncaughtException(it, e) }
    }
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + unopened)
    private val opened: Deferred<KoanEngine> = scope.async {
        try {
            KoanEngine.open(deviceName)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            _startFailure.value = failureToStart(e)
            throw e
        }
    }

    private val _nowPlaying = MutableStateFlow<NowPlaying?>(null)
    /** Everything a transport shows but the position. `null` until the engine first answers. */
    val nowPlaying: StateFlow<NowPlaying?> = _nowPlaying.asStateFlow()

    private val _playhead = MutableStateFlow(Playhead(0, 0, false, 0))
    val playhead: StateFlow<Playhead> = _playhead.asStateFlow()

    private val _queue = MutableStateFlow(QueueState(emptyList(), 0))
    val queue: StateFlow<QueueState> = _queue.asStateFlow()

    private val _lock = MutableStateFlow<QueueLock?>(null)
    /** What the queue still is, when it is still a playlist or a record. */
    val lock: StateFlow<QueueLock?> = _lock.asStateFlow()

    private val _transfers = MutableStateFlow<List<Transfer>>(emptyList())
    val transfers: StateFlow<List<Transfer>> = _transfers.asStateFlow()

    private val _figures = MutableStateFlow<List<TransferFigure>>(emptyList())
    /** Byte counts behind `transfers`; the fast half. */
    val figures: StateFlow<List<TransferFigure>> = _figures.asStateFlow()

    private val _library = MutableStateFlow(0L)
    val library: StateFlow<Long> = _library.asStateFlow()

    private val _history = MutableStateFlow(0L)
    val history: StateFlow<Long> = _history.asStateFlow()

    private val _cacheBytes = MutableStateFlow(0L)
    val cacheBytes: StateFlow<Long> = _cacheBytes.asStateFlow()

    private val _tasks = MutableStateFlow(Tasks(scanning = false, syncing = false))
    val tasks: StateFlow<Tasks> = _tasks.asStateFlow()

    private val _sync = MutableStateFlow<SyncProgress?>(null)
    val sync: StateFlow<SyncProgress?> = _sync.asStateFlow()

    private val _devices = MutableStateFlow(Devices(emptyList(), null))
    /** Devices to play on, and the one controlled: `null` for this one. */
    val devices: StateFlow<Devices> = _devices.asStateFlow()

    private val _connection = MutableStateFlow<ConnectionInfo?>(null)
    val connection: StateFlow<ConnectionInfo?> = _connection.asStateFlow()

    private val _notices = MutableSharedFlow<String>(extraBufferCapacity = 8)
    /** What the person is told once and then not again: a command that went nowhere, and why. */
    val notices: SharedFlow<String> = _notices
    private var noticeSeq = 0uL

    private val _renderers = MutableStateFlow(Renderers(emptyList(), null))
    val renderers: StateFlow<Renderers> = _renderers.asStateFlow()

    private val _outputs = MutableStateFlow<OutputsInfo?>(null)
    val outputs: StateFlow<OutputsInfo?> = _outputs.asStateFlow()

    private val _appearance = MutableStateFlow<Appearance?>(null)
    /**
     * How the app is drawn, from `[appearance]`. Not a slice: read as the
     * engine opens, and again by `setAppearance` after each change.
     */
    val appearance: StateFlow<Appearance?> = _appearance.asStateFlow()

    /**
     * Appearance changes, handed to the engine in the order they were made,
     * which is the order it saves them in. The one coroutine that reads them
     * is also the only writer of [appearance], so the first read cannot land
     * over a change.
     */
    private val appearanceChanges = Channel<KoanEngine.() -> Unit>(Channel.UNLIMITED).also { changes ->
        // On IO: a read after a change waits for it to reach the disk.
        scope.launch(Dispatchers.IO) {
            val engine = opened.await()
            var change: (KoanEngine.() -> Unit)? = null
            while (true) {
                try {
                    change?.invoke(engine)
                    _appearance.value = engine.appearance()
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    report(e)
                }
                change = changes.receive()
            }
        }
    }

    /** The queue as last sent whole, and where each row sits in it. */
    private var base = QueueState(emptyList(), 0)
    private var baseIndex = emptyMap<String, Int>()

    init {
        scope.launch {
            val stream = opened.await().observe()
            while (true) {
                val slices = stream.next() ?: break
                slices.forEach(::apply)
            }
        }
    }

    /** The engine, once it has opened the library and started the player. */
    suspend fun engine(): KoanEngine = opened.await()

    /**
     * Run `block` against the engine off the main thread, without waiting for
     * it. A failure is told to the person, never dropped: a call that may fail
     * quietly catches its own.
     */
    fun launch(block: suspend KoanEngine.() -> Unit) {
        scope.launch {
            try {
                opened.await().block()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                report(e)
            }
        }
    }

    /** Tell the person why something they asked for did not happen. */
    fun report(e: Exception) {
        // The whole app says why it could not start; each call need not.
        if (_startFailure.value != null) return
        _notices.tryEmit((e as? KoanException)?.describe() ?: "Something went wrong: ${e.message ?: e}")
    }

    /** Change an appearance setting (`setTheme`, `setThemeIcons`, `setRecordColours`), and redraw in it. */
    fun setAppearance(change: KoanEngine.() -> Unit) {
        appearanceChanges.trySend(change)
    }

    private fun apply(slice: StateSlice) {
        when (slice) {
            is StateSlice.Playback -> {
                val failure = slice.nowPlaying.startFailure
                if (failure != null && failure != _nowPlaying.value?.startFailure) _notices.tryEmit(failure)
                _nowPlaying.value = slice.nowPlaying
            }
            is StateSlice.Playhead -> _playhead.value = Playhead(
                slice.positionMs.toLong(),
                slice.seekableMs.toLong(),
                slice.playing,
                SystemClock.elapsedRealtime(),
            )
            is StateSlice.Queue -> {
                base = QueueState(slice.items, slice.version.toLong())
                baseIndex = slice.items.withIndex().associate { (i, row) -> row.queueItemId to i }
                _queue.value = base
            }
            // Relative to the base, so applying it to the base is all there is.
            is StateSlice.QueuePatch -> if (slice.base.toLong() == base.version) {
                val rows = base.items.toMutableList()
                slice.items.forEach { row -> baseIndex[row.queueItemId]?.let { rows[it] = row } }
                _queue.value = QueueState(rows, base.version)
            }
            is StateSlice.Lock -> _lock.value = slice.lock
            is StateSlice.Transfers -> _transfers.value = slice.transfers
            is StateSlice.Figures -> _figures.value = slice.figures
            is StateSlice.Library -> _library.value = slice.version.toLong()
            is StateSlice.History -> _history.value = slice.version.toLong()
            is StateSlice.Cache -> _cacheBytes.value = slice.bytes.toLong()
            is StateSlice.Tasks -> _tasks.value = Tasks(slice.scanning, slice.syncing)
            is StateSlice.Sync -> _sync.value = slice.progress
            is StateSlice.Devices -> _devices.value = Devices(slice.devices, slice.target, slice.targetReach)
            is StateSlice.Connection -> {
                _connection.value = slice.connection
                // A command already on its way that then did not arrive.
                slice.connection.commandNotice?.takeIf { it.seq > noticeSeq }?.let { n ->
                    noticeSeq = n.seq
                    _notices.tryEmit(
                        when {
                            n.queued -> "Waiting for ${n.device} to wake"
                            n.refused -> "${n.device} turned it down: ${n.detail}"
                            n.unreached -> n.detail
                            else -> "${n.device} couldn't do it: ${n.detail}"
                        },
                    )
                }
            }
            is StateSlice.Renderers -> _renderers.value = Renderers(slice.renderers, slice.output)
            is StateSlice.Outputs -> _outputs.value = slice.outputs
        }
    }
}

/** The engine's own words for what went wrong. */
/** What the app says when the engine would not open. */
fun failureToStart(e: Exception) = (e as? KoanException)?.describe() ?: e.message ?: e.toString()

fun KoanException.describe() = when (this) {
    is KoanException.Database -> detail
    is KoanException.Audio -> detail
    is KoanException.Player -> detail
    is KoanException.NotFound -> detail
    is KoanException.BadArgument -> detail
    is KoanException.Remote -> detail
    is KoanException.NeedsSampleRate -> detail
}

package cc.blit.koan.engine

import android.os.SystemClock
import cc.blit.koan.ffi.ConnectionInfo
import cc.blit.koan.ffi.DeviceInfo
import cc.blit.koan.ffi.KoanEngine
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
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
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
    fun positionAt(nowMs: Long = SystemClock.elapsedRealtime()) =
        if (playing) positionMs + (nowMs - atMs) else positionMs
}

/** The queue as the engine last sent it, with the latest row patch applied. */
data class QueueState(val items: List<QueueItem>, val version: Long)

data class Tasks(val scanning: Boolean, val syncing: Boolean)

data class Devices(val devices: List<DeviceInfo>, val target: String?)

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
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val opened: Deferred<KoanEngine> = scope.async { KoanEngine.open(deviceName) }

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

    private val _renderers = MutableStateFlow(Renderers(emptyList(), null))
    val renderers: StateFlow<Renderers> = _renderers.asStateFlow()

    private val _outputs = MutableStateFlow<OutputsInfo?>(null)
    val outputs: StateFlow<OutputsInfo?> = _outputs.asStateFlow()

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

    /** Run `block` against the engine off the main thread, without waiting for it. */
    fun launch(block: suspend KoanEngine.() -> Unit) {
        scope.launch { runCatching { opened.await().block() } }
    }

    private fun apply(slice: StateSlice) {
        when (slice) {
            is StateSlice.Playback -> _nowPlaying.value = slice.nowPlaying
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
            is StateSlice.Devices -> _devices.value = Devices(slice.devices, slice.target)
            is StateSlice.Connection -> _connection.value = slice.connection
            is StateSlice.Renderers -> _renderers.value = Renderers(slice.renderers, slice.output)
            is StateSlice.Outputs -> _outputs.value = slice.outputs
        }
    }
}

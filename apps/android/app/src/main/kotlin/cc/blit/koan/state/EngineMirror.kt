package cc.blit.koan.state

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.koan_ffi.ConnectionInfo
import uniffi.koan_ffi.DeviceInfo
import uniffi.koan_ffi.EntryStatus
import uniffi.koan_ffi.NowPlaying
import uniffi.koan_ffi.OutputsInfo
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.QueueItem
import uniffi.koan_ffi.QueueLock
import uniffi.koan_ffi.RendererInfo
import uniffi.koan_ffi.RendererOutput
import uniffi.koan_ffi.RepeatMode
import uniffi.koan_ffi.StateSlice
import uniffi.koan_ffi.StateStreamInterface
import uniffi.koan_ffi.SyncProgress
import uniffi.koan_ffi.Transfer
import uniffi.koan_ffi.TransferFigure

/**
 * The engine's state, as Compose and the playback service see it.
 *
 * One object, one loop, no per-model refresh rules. The engine publishes whole
 * slices and this holds the latest of each, one [StateFlow] per slice. Nothing
 * here decides *what* a change means: a slice arrives only when it differs,
 * and the difference is decided in Rust, where every one of these types
 * already has an equality of its own. A snapshot cannot be applied wrongly.
 * (This is the Kotlin port of `EngineMirror.swift`, and the doc comments
 * there and in `crates/koan-ffi/src/state.rs` are its specification.)
 *
 * What a [StateFlow] adds is the check the Swift side gets from observation:
 * a value equal to the one held is not a change, so a collector is not
 * woken. Equality is structural for small values and by identity for the big
 * lists ([QueueRows], [QueueState]), which are only ever built because the
 * engine said they differ.
 *
 * Two things decide the shape, as in Swift:
 *
 * 1. **Granularity is per flow.** Collecting `queue` does not subscribe you to
 *    `playhead`. Slices are cut by *rate of change*: the position and the
 *    transfer figures move ten times a second and have flows of their own, so
 *    a list that draws neither never hears from them.
 * 2. **It is not per element.** A new `queue` wakes everyone collecting it,
 *    so anything that moves at its own rate has left the list rather than
 *    being patched into it: a transfer's byte count is not a field on a row.
 *
 * **Threading.** [apply] and [follow] run on one coroutine, which the caller
 * puts on the engine dispatcher, never Main; the private working state is
 * confined to it. Every flow may be collected from any thread.
 * Collect with `collectAsStateWithLifecycle` so a backgrounded screen stops
 * collecting and nothing ticks for it.
 */
class EngineMirror(private val nanoClock: () -> Long = System::nanoTime) {

    // MARK: - Slices

    private val _playback = MutableStateFlow(idle)
    private val _playhead = MutableStateFlow(Playhead.Stopped)
    private val _seekableMs = MutableStateFlow(0L)
    private val _queueRows = MutableStateFlow(QueueRows.Empty)
    private val _queue = MutableStateFlow(QueueState.Empty)
    private val _queueIndex = MutableStateFlow(QueueIndex.Empty)
    private val _lock = MutableStateFlow<QueueLock?>(null)
    private val _transfers = MutableStateFlow<List<Transfer>>(emptyList())
    private val _figures = MutableStateFlow<Map<Long, TransferFigure>>(emptyMap())
    private val _libraryVersion = MutableStateFlow(0L)
    private val _historyVersion = MutableStateFlow(0L)
    private val _cacheBytes = MutableStateFlow(0L)
    private val _tasks = MutableStateFlow(EngineTasks(scanning = false, syncing = false))
    private val _syncProgress = MutableStateFlow<SyncProgress?>(null)
    private val _devices = MutableStateFlow(DevicesState.None)
    private val _connection = MutableStateFlow<ConnectionInfo?>(null)
    private val _renderers = MutableStateFlow(RenderersState.None)
    private val _outputs = MutableStateFlow<OutputsInfo?>(null)
    private val _batches = MutableStateFlow(0L)

    /**
     * Everything a transport bar shows other than the position. Changes per
     * track, per state, per format — not per tick.
     */
    val playback: StateFlow<NowPlaying> = _playback.asStateFlow()

    /**
     * Where the playhead was when the engine last said, and whether it has
     * been moving since. An anchor rather than a reading: ask it for
     * [Playhead.at] rather than expecting a number that would have to keep
     * arriving to stay true.
     */
    val playhead: StateFlow<Playhead> = _playhead.asStateFlow()

    /**
     * How far into the current track a seek can land. Equal to the duration
     * for anything on disk, and short of it while a download is still
     * arriving — the engine clamps a seek to the same extent, so a scrub past
     * this would land somewhere the thumb was never dragged to.
     */
    val seekableMs: StateFlow<Long> = _seekableMs.asStateFlow()

    /** What koan is doing on its own. One flow for both: the only reader draws a row for each. */
    val tasks: StateFlow<EngineTasks> = _tasks.asStateFlow()

    /** How far the running remote sync has got, whoever started it. Apart from [tasks]: it moves once per page. */
    val syncProgress: StateFlow<SyncProgress?> = _syncProgress.asStateFlow()

    /** The queue as it reads now: [queueRows] with the latest patch applied. */
    val queue: StateFlow<QueueState> = _queue.asStateFlow()

    /**
     * The queue's rows as last sent whole. Moves when the queue is edited,
     * and not when a track changes or a download lands. Its `version` moves
     * with it and means the same.
     */
    val queueRows: StateFlow<QueueRows> = _queueRows.asStateFlow()

    /** Entries by library track and by playlist row; moves with [queue]. */
    val queueIndex: StateFlow<QueueIndex> = _queueIndex.asStateFlow()

    /** What the queue still is, while it is still a playlist or a record. */
    val lock: StateFlow<QueueLock?> = _lock.asStateFlow()

    /** Every transfer koan knows about — running first, then what settled most recently. Structural: see [figures]. */
    val transfers: StateFlow<List<Transfer>> = _transfers.asStateFlow()

    /**
     * The byte counts behind the transfers, keyed by track. The fast half:
     * moves ten times a second while anything downloads, so collect it only
     * where a ring or a bar is drawn.
     */
    val figures: StateFlow<Map<Long, TransferFigure>> = _figures.asStateFlow()

    /**
     * Bumped whenever the library's rows change — a scan, a sync, an import,
     * an organize, a playlist edit, a download landing.
     *
     * A signal, not a mirror. A record's tracks and a search's results are
     * asked for on demand, so what they want is to know to ask again: a page
     * keys its load on this. The page that draws a thing is the thing that
     * reloads it, so there is no model to forget one.
     */
    val libraryVersion: StateFlow<Long> = _libraryVersion.asStateFlow()

    /** Moves when a play is recorded or plays are forgotten: pages derived from history ask again. */
    val historyVersion: StateFlow<Long> = _historyVersion.asStateFlow()

    /** Bytes in the download cache, kept by the engine as downloads land and leave. */
    val cacheBytes: StateFlow<Long> = _cacheBytes.asStateFlow()

    /**
     * The other devices koan can play on, and the one this app controls (null
     * for this one). While it is another, [playback], [playhead] and [queue]
     * are that device's.
     */
    val devices: StateFlow<DevicesState> = _devices.asStateFlow()

    /** The server and the network, as far as devices go. */
    val connection: StateFlow<ConnectionInfo?> = _connection.asStateFlow()

    /** UPnP renderers, and the one playing this device's music in place of its own output. */
    val renderers: StateFlow<RenderersState> = _renderers.asStateFlow()

    /** What the device in view plays through; null while a controlled device has said nothing. */
    val outputs: StateFlow<OutputsInfo?> = _outputs.asStateFlow()

    /** Counts the engine's batches applied. Ticks once per batch, for [waitUntil] and tests. */
    val batches: StateFlow<Long> = _batches.asStateFlow()

    /**
     * Whether the signed-in server lists the OpenSubsonic extension `name`:
     * what decides which of koan's own features a page offers.
     */
    fun offers(name: String): Boolean =
        _connection.value?.extensions?.any { it.name == name } == true

    /**
     * The server refused the credential this device signs in with: a revoked
     * key or a changed password. Until someone signs in again nothing more
     * arrives from it, so an empty page says this rather than waiting.
     */
    val signInRefused: Boolean get() = _connection.value?.signInRefused == true

    /** How many transfers are moving. What the sidebar counts. */
    val activeTransfers: Int get() = _transfers.value.count { !it.state.isSettled }

    // MARK: - Following

    /**
     * Follow the engine until [stream] ends (the engine is gone) or the
     * calling coroutine is cancelled. A batch is everything that changed
     * inside one of the engine's ticks, so a frame's worth of movement lands
     * as one pass.
     *
     * The loop *is* the subscription: there is nothing to register and
     * nothing to unregister. Run it on the engine dispatcher.
     */
    suspend fun follow(stream: StateStreamInterface) {
        while (true) {
            val batch = stream.next() ?: return
            for (slice in batch) apply(slice)
            _batches.value += 1
        }
    }

    /**
     * Take a slice. Every one that arrives differs from the one held (the
     * engine already decided that), so this is an assignment and never a
     * merge. The few that are cheap to compare and often equal in practice
     * are checked anyway, as in Swift, and the flows drop an equal value.
     */
    internal fun apply(slice: StateSlice) {
        when (slice) {
            is StateSlice.Playback -> _playback.value = slice.nowPlaying
            is StateSlice.Playhead -> {
                // Stamped on arrival. The engine cannot send a clock across the
                // boundary and does not need to: the message is minutes old only
                // if it sat in a queue, and it did not.
                _playhead.value = Playhead(slice.positionMs.toLong(), slice.playing, nanoClock())
                _seekableMs.value = slice.seekableMs.toLong()
            }
            is StateSlice.Queue -> replaceQueue(slice.items, slice.version.toLong())
            is StateSlice.QueuePatch -> patchQueue(slice.base.toLong(), slice.items)
            is StateSlice.Lock -> _lock.value = slice.lock
            is StateSlice.Transfers -> _transfers.value = slice.transfers
            is StateSlice.Figures -> _figures.value = slice.figures.associateBy { it.trackId }
            is StateSlice.Library -> _libraryVersion.value = slice.version.toLong()
            is StateSlice.History -> _historyVersion.value = slice.version.toLong()
            is StateSlice.Cache -> _cacheBytes.value = slice.bytes.toLong()
            is StateSlice.Tasks -> _tasks.value = EngineTasks(slice.scanning, slice.syncing)
            is StateSlice.Sync -> _syncProgress.value = slice.progress
            is StateSlice.Devices ->
                _devices.value = DevicesState(slice.devices, slice.target, nanoClock())
            is StateSlice.Connection -> _connection.value = slice.connection
            is StateSlice.Renderers -> _renderers.value = RenderersState(slice.renderers, slice.output)
            is StateSlice.Outputs -> _outputs.value = slice.outputs
        }
    }

    // MARK: - The queue
    //
    // Working state, confined to the following coroutine.

    private var base: List<QueueItem> = emptyList()
    private var rowOf: Map<String, Int> = emptyMap()
    private var rowsByTrack: Map<Long, List<Int>> = emptyMap()
    private var rowsByPlaylistEntry: Map<Long, List<Int>> = emptyMap()

    /** The rows the latest patch changed, to put back before the next. */
    private var patched: List<Int> = emptyList()
    private var version = 0L

    private fun replaceQueue(items: List<QueueItem>, newVersion: Long) {
        base = items
        version = newVersion
        patched = emptyList()
        val at = HashMap<String, Int>(items.size * 2)
        val byTrack = HashMap<Long, MutableList<Int>>()
        val byEntry = HashMap<Long, MutableList<Int>>()
        items.forEachIndexed { row, item ->
            at.putIfAbsent(item.queueItemId, row)
            item.trackId?.let { byTrack.getOrPut(it) { mutableListOf() }.add(row) }
            item.playlistEntryId?.let { byEntry.getOrPut(it) { mutableListOf() }.add(row) }
        }
        rowOf = at
        rowsByTrack = byTrack
        rowsByPlaylistEntry = byEntry
        _queueRows.value = QueueRows(items, newVersion)
        _queue.value = QueueState(items, newVersion, at)
        _queueIndex.value = QueueIndex(
            byTrack.mapValues { (_, rows) -> preferred(rows.map { items[it] })!! },
            byEntry.mapValues { (_, rows) -> preferred(rows.map { items[it] })!! },
        )
    }

    private fun patchQueue(patchBase: Long, items: List<QueueItem>) {
        // A patch to a queue that has been sent whole since is stale; and an
        // empty one on an unpatched queue says nothing.
        if (patchBase != version || (items.isEmpty() && patched.isEmpty())) return
        val rows = ArrayList(base)
        val touched = ArrayList(patched)
        val nowPatched = ArrayList<Int>(items.size)
        for (item in items) {
            val row = rowOf[item.queueItemId] ?: continue
            rows[row] = item
            nowPatched.add(row)
            touched.add(row)
        }
        patched = nowPatched
        // A patch never changes which track or playlist entry a row is, so
        // only those indexes need asking again.
        val index = _queueIndex.value
        val byTrack = HashMap(index.byTrack)
        val byEntry = HashMap(index.byPlaylistEntry)
        for (row in touched) {
            val item = rows[row]
            item.trackId?.let { track ->
                rowsByTrack[track]?.let { held -> preferred(held.map { rows[it] })?.let { byTrack[track] = it } }
            }
            item.playlistEntryId?.let { entry ->
                rowsByPlaylistEntry[entry]?.let { held -> preferred(held.map { rows[it] })?.let { byEntry[entry] = it } }
            }
        }
        _queue.value = QueueState(rows, version, rowOf)
        _queueIndex.value = QueueIndex(byTrack, byEntry)
    }

    // MARK: - Waiting

    /**
     * Wait until `settled` holds, or until `timeoutMs` passes; true if it held.
     *
     * For the places that have asked the engine for something and want the
     * answer before drawing: a restored queue, a queue mutation to confirm.
     * The wait is woken by a batch arriving, and the deadline is one timeout
     * for the whole wait.
     */
    suspend fun waitUntil(timeoutMs: Long, settled: () -> Boolean): Boolean =
        settled() || withTimeoutOrNull(timeoutMs) {
            batches.first { settled() }
            true
        } == true

    companion object {
        /** What a mirror reads before the engine has said anything. */
        val idle = NowPlaying(
            state = PlayState.STOPPED, waiting = false, positionMs = 0uL, durationMs = 0uL,
            queueItemId = null, entry = null, format = null, playlistVersion = 0uL,
            shuffle = false, repeatMode = RepeatMode.OFF, sleep = null, sleepFading = false,
        )
    }
}

/** The other devices, as they were when this arrived (a playing device's playhead runs on from [atNanos]). */
data class DevicesState(val devices: List<DeviceInfo>, val target: String?, val atNanos: Long) {
    val controlled: DeviceInfo? get() = target?.let { id -> devices.firstOrNull { it.id == id } }

    companion object {
        val None = DevicesState(emptyList(), null, 0)
    }
}

data class RenderersState(val renderers: List<RendererInfo>, val output: RendererOutput?) {
    companion object {
        val None = RenderersState(emptyList(), null)
    }
}

/** A transfer that has ended, one way or the other. */
val uniffi.koan_ffi.TransferState.isSettled: Boolean
    get() = this == uniffi.koan_ffi.TransferState.DONE || this == uniffi.koan_ffi.TransferState.FAILED

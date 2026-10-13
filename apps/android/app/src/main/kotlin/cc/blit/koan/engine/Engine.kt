package cc.blit.koan.engine

import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.NowPlaying
import cc.blit.koan.ffi.QueueItem
import cc.blit.koan.ffi.StateSlice
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** Where the playhead is, and whether it is moving. */
data class Playhead(val positionMs: Long, val playing: Boolean)

/**
 * The one `KoanEngine` in the process, and its state as flows.
 *
 * The engine publishes whole slices through a cursor (`KoanEngine.observe`);
 * one loop reads them and sets each flow, so a collector only ever sees the
 * latest value of what it reads. Collectors on the main thread get it there:
 * the loop runs off it, and a `StateFlow` hands values to each collector on
 * that collector's own dispatcher.
 */
class Engine(deviceName: String) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private val opened: Deferred<KoanEngine> = scope.async { KoanEngine.open(deviceName) }

    private val _nowPlaying = MutableStateFlow<NowPlaying?>(null)
    val nowPlaying: StateFlow<NowPlaying?> = _nowPlaying.asStateFlow()

    private val _playhead = MutableStateFlow(Playhead(0, false))
    val playhead: StateFlow<Playhead> = _playhead.asStateFlow()

    private val _queue = MutableStateFlow<List<QueueItem>>(emptyList())
    val queue: StateFlow<List<QueueItem>> = _queue.asStateFlow()

    init {
        scope.launch {
            val stream = opened.await().observe()
            while (true) {
                val slices = stream.next() ?: break
                for (slice in slices) {
                    when (slice) {
                        is StateSlice.Playback -> _nowPlaying.value = slice.nowPlaying
                        is StateSlice.Playhead ->
                            _playhead.value = Playhead(slice.positionMs.toLong(), slice.playing)
                        is StateSlice.Queue -> _queue.value = slice.items
                        else -> {}
                    }
                }
            }
        }
    }

    /** The engine, once it has opened the library and started the player. */
    suspend fun get(): KoanEngine = opened.await()

    /** Run `block` against the engine without waiting for it. */
    fun launch(block: suspend KoanEngine.() -> Unit) {
        scope.launch { opened.await().block() }
    }
}

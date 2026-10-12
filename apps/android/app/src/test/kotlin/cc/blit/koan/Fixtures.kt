package cc.blit.koan

import kotlinx.coroutines.channels.Channel
import uniffi.koan_ffi.EntryStatus
import uniffi.koan_ffi.KoanEngineInterface
import uniffi.koan_ffi.NowPlaying
import uniffi.koan_ffi.PlayState
import uniffi.koan_ffi.QueueItem
import uniffi.koan_ffi.RepeatMode
import uniffi.koan_ffi.StateSlice
import uniffi.koan_ffi.StateStreamInterface
import uniffi.koan_ffi.StreamFormat
import java.lang.reflect.Proxy
import kotlin.coroutines.Continuation

fun item(
    id: String,
    track: Long? = null,
    entry: Long? = null,
    status: EntryStatus = EntryStatus.QUEUED,
    title: String = "Title $id",
) = QueueItem(
    queueItemId = id, trackId = track, albumId = null, title = title, artist = "Artist", albumArtist = "Artist",
    album = "Album", year = null, codec = "FLAC", trackNumber = 1L, disc = 1L, durationMs = 180_000uL,
    status = status, playlistEntryId = entry, failureReason = null, onServer = true, onDisk = true,
)

fun nowPlaying(
    state: PlayState = PlayState.STOPPED,
    entry: QueueItem? = null,
    waiting: Boolean = false,
    durationMs: ULong = 180_000uL,
    shuffle: Boolean = false,
    repeat: RepeatMode = RepeatMode.OFF,
    format: StreamFormat? = null,
) = NowPlaying(
    state = state, waiting = waiting, positionMs = 0uL, durationMs = durationMs,
    queueItemId = entry?.queueItemId, entry = entry, format = format, playlistVersion = 0uL,
    shuffle = shuffle, repeatMode = repeat, sleep = null, sleepFading = false,
)

fun format(rate: UInt = 44100u) = StreamFormat(
    codec = "FLAC", sampleRate = rate, bitDepth = 16u.toUShort(), bitrateKbps = null, channels = 2u.toUShort(),
    outputSampleRate = null, dsp = null,
)

/** A stream the test feeds by hand: `next()` suspends until [send], and ends on [close]. */
class FakeStream : StateStreamInterface {
    private val batches = Channel<List<StateSlice>>(Channel.UNLIMITED)
    fun send(vararg slices: StateSlice) { batches.trySend(slices.toList()) }
    fun close() { batches.close() }
    override suspend fun next(): List<StateSlice>? = batches.receiveCatching().getOrNull()
}

/**
 * An engine that records the calls made to it. Every method of the generated
 * interface is answered by [answer] (default: Unit), so a test names only the
 * ones it cares about.
 */
class FakeEngine(private val answer: (String, List<Any?>) -> Any? = { _, _ -> Unit }) {
    val calls = mutableListOf<String>()

    val engine: KoanEngineInterface = Proxy.newProxyInstance(
        KoanEngineInterface::class.java.classLoader, arrayOf(KoanEngineInterface::class.java),
    ) { _, method, args ->
        // Suspend functions carry the Continuation as their last parameter.
        val real = args.orEmpty().filter { it !is Continuation<*> }
        val name = method.name.substringBefore('-') // value-class parameters mangle the name
        synchronized(calls) { calls += if (real.isEmpty()) name else "$name(${real.joinToString()})" }
        when (name) {
            "toString" -> "FakeEngine"
            "hashCode" -> 0
            "equals" -> false
            else -> defaults(name) ?: answer(name, real)
        }
    } as KoanEngineInterface

    private fun defaults(name: String): Any? = when (name) {
        "restoreSession" -> 0u
        "addToQueue", "insertAfter", "replaceQueue" -> emptyList<String>()
        "track" -> null
        else -> null
    }
}

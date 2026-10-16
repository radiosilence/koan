package cc.blit.koan

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
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

fun album(id: Long, title: String = "Album $id", artist: Long = 1) = uniffi.koan_ffi.Album(
    id = id, title = title, artistId = artist, artistName = "Artist $artist", year = 2000, codec = "FLAC",
    label = null, totalDiscs = 1, totalTracks = 10, addedAt = null, onDevice = null,
)

fun artist(id: Long) = uniffi.koan_ffi.Artist(id = id, name = "Artist $id", sortName = null, albumCount = 1, trackCount = 10)

fun track(id: Long, album: Long? = 1) = uniffi.koan_ffi.Track(
    id = id, title = "Track $id", artistName = "Artist 1", albumArtistName = "Artist 1", albumTitle = "Album 1",
    albumId = album, artistId = 1, disc = 1, trackNumber = id.toInt(), durationMs = 180_000, codec = "FLAC",
    sampleRate = 44100, bitDepth = 16, channels = 2, bitrate = null, genre = null, source = "local",
    onServer = false, onDisk = true, path = null, isFavourite = false,
)

fun playlist(id: Long, readonly: Boolean = false, changedAt: String = "t0", fromFile: Boolean = false) = uniffi.koan_ffi.Playlist(
    id = id, name = "Playlist $id", comment = null, remoteId = null, public = false, owner = null, trackCount = 0u,
    durationMs = 0, createdAt = "t0", changedAt = changedAt, grouped = null, readonly = readonly, smart = false,
    fromFile = fromFile,
)

/**
 * Starts collecting [this] eagerly into a list: every value the flow delivers
 * from now on, the one it holds first. Cancel with the scope.
 */
fun <T> kotlinx.coroutines.flow.StateFlow<T>.record(scope: kotlinx.coroutines.CoroutineScope): List<T> {
    val seen = java.util.concurrent.CopyOnWriteArrayList<T>()
    scope.launch(kotlinx.coroutines.Dispatchers.Unconfined) { collect { seen += it } }
    return seen
}

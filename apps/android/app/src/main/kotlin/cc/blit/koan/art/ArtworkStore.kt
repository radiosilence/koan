package cc.blit.koan.art

import cc.blit.koan.theme.KoanAccent
import cc.blit.koan.theme.SleeveColour
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Whose artwork: the record's, a track's, or an artist's photograph. */
sealed interface ArtworkSource {
    /** What the caches key on. One blob per record, whichever track asked. */
    val key: String

    /** A record's sleeve. Every track on it shares this one. */
    data class Album(val id: Long) : ArtworkSource {
        override val key: String get() = "album:$id"
    }

    /** A track's own cover (embedded art can differ from the record's). */
    data class Track(val id: Long) : ArtworkSource {
        override val key: String get() = "track:$id"
    }

    data class Artist(val id: Long) : ArtworkSource {
        override val key: String get() = "artist:$id"
    }
}

/** Image bytes and what they are. */
class ArtworkBytes(val data: ByteArray, val mime: String)

/** Where bytes come from: the engine. A seam so the store is tested without one. */
fun interface CoverBytes {
    /** The artwork at about [pixels] on its long side, or null if there is none. Throws if the lookup failed. */
    suspend fun fetch(source: ArtworkSource, pixels: UInt): ArtworkBytes?
}

/** Anything that can be told to have a sleeve ready. The library model asks this of the store, never of the engine. */
fun interface ArtworkWarmer {
    fun warm(source: ArtworkSource)
}

/**
 * The bytes of artwork, fetched once per record: the Kotlin replacement for the
 * fetch half of `CoverArtCache.swift`, whose doc comments are the specification.
 * Bitmaps are Coil's (a bounded memory cache keyed by record and draw size, and
 * a disk cache of these bytes: see `ArtImageLoader`); this is what sits
 * behind them, on a miss.
 *
 * Bytes are fetched once per record. On a remote-backed library every miss is
 * an HTTP round trip, and keyed per track the same sleeve would be fetched once
 * for every track on the record. So concurrent callers share one fetch, and a
 * record found to have no art is remembered as having none ([isAbsent]), distinct
 * from "not looked yet", which is what keeps missing art from being retried on
 * every redraw. A failed lookup (the network was down) is not remembered.
 *
 * Runs on [dispatcher], the IO pool, never Main and never the engine's own
 * dispatcher: a grid of twenty tiles is twenty independent lookups, and the
 * engine does them on its own threads.
 *
 * Also works out the colour a record reads as ([accent]), once per record, from
 * the same bytes.
 */
class ArtworkStore(
    private val covers: CoverBytes,
    private val colourOf: (ByteArray) -> Int? = { null },
    private val dispatcher: CoroutineDispatcher = Dispatchers.IO,
    private val scope: CoroutineScope = CoroutineScope(SupervisorJob() + dispatcher),
    private val maxColours: Int = 512,
) : ArtworkWarmer {
    private val lock = Any()
    private val absent = HashSet<String>()
    private val loads = HashMap<String, CompletableDeferred<ArtworkBytes?>>()

    /** Record colour by source key, most recently used last. */
    private val colours = object : LinkedHashMap<String, Int?>(16, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Int?>?): Boolean = size > maxColours
    }

    /** Whether the record has been looked at and has no art. False is also a record not looked at yet. */
    fun isAbsent(source: ArtworkSource): Boolean = synchronized(lock) { source.key in absent }

    /**
     * The record's bytes at [SOURCE_PIXELS], awaiting the engine. One lookup is
     * shared by every caller asking for the same record at once. Null: no art.
     * Throws if the lookup itself failed, which is not remembered.
     */
    suspend fun bytes(source: ArtworkSource): ArtworkBytes? {
        val (shared, starts) = synchronized(lock) {
            if (source.key in absent) return null
            val running = loads[source.key]
            if (running != null) {
                running to false
            } else {
                val fresh = CompletableDeferred<ArtworkBytes?>()
                loads[source.key] = fresh
                fresh to true
            }
        }
        // The fetch belongs to the store, not to whoever asked first: a tile
        // scrolled away mid-fetch must not cancel it for the tile that wants it.
        if (starts) scope.launch { run(source, shared) }
        return shared.await()
    }

    private suspend fun run(source: ArtworkSource, into: CompletableDeferred<ArtworkBytes?>) {
        try {
            val found = withContext(dispatcher) { covers.fetch(source, SOURCE_PIXELS) }
            synchronized(lock) {
                if (found == null) absent += source.key
                loads.remove(source.key)
            }
            into.complete(found)
        } catch (e: CancellationException) {
            synchronized(lock) { loads.remove(source.key) }
            into.cancel(e)
        } catch (e: Exception) {
            synchronized(lock) { loads.remove(source.key) }
            into.completeExceptionally(e)
        }
    }

    /** Start the fetch for a record the user is about to see, and nothing else. Never waited on. */
    override fun warm(source: ArtworkSource) {
        if (isAbsent(source)) return
        scope.launch {
            try {
                bytes(source)
                accent(source)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // Warming is a courtesy: the page that draws it asks again.
            }
        }
    }

    /**
     * The accent a record gives, mint when it has no art or no colour. Worked
     * out once per record, from the bytes the sleeve already needed. A failed
     * lookup is mint for now and worked out again next time.
     */
    suspend fun accent(source: ArtworkSource): KoanAccent {
        val known = synchronized(lock) { if (colours.containsKey(source.key)) colours[source.key] to true else null to false }
        if (known.second) return KoanAccent.of(known.first)
        val bytes = try {
            bytes(source)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            return KoanAccent.Mint
        }
        val colour = bytes?.let { withContext(dispatcher) { colourOf(it.data) } }
        synchronized(lock) { colours[source.key] = colour }
        return KoanAccent.of(colour)
    }

    /**
     * Rows were added or removed under the library: a record that had no art may
     * have some now. Remembers nothing absent; held colours stay (they are of
     * art, and art does not change under a record id).
     */
    fun libraryChanged() = synchronized(lock) { absent.clear() }

    companion object {
        /**
         * What we ask the engine for, and what lands on disk. One size serves the
         * grid, the rows and the viewer, because there is one blob per record:
         * large enough that the opened sleeve holds up on a dense display, small
         * enough that a library's worth of them is a cache rather than a second
         * copy of the artwork.
         */
        const val SOURCE_PIXELS: UInt = 1000u

        /** Pixel sizes a draw is rounded up to, so the memory cache holds a few bitmaps per record and not one per layout. */
        val Buckets = intArrayOf(64, 128, 256, 512, SOURCE_PIXELS.toInt())

        /** The bucket a draw of [px] pixels is read at: the smallest that is not smaller than it. */
        fun bucket(px: Int): Int = Buckets.firstOrNull { it >= px } ?: Buckets.last()
    }
}

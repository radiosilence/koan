package cc.blit.koan.art

import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.CoverArt
import coil3.ImageLoader
import coil3.decode.DataSource
import coil3.decode.ImageSource
import coil3.fetch.FetchResult
import coil3.fetch.Fetcher
import coil3.fetch.SourceFetchResult
import coil3.key.Keyer
import coil3.request.Options
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import okio.Buffer

/** How big a cover is wanted: a thumbnail for lists and grids, or the whole image. */
enum class ArtSize(val px: UInt?) {
    Thumb(300u),
    Full(null),
}

/**
 * A cover, as Coil's `model`: `AsyncImage(model = Art.album(id), …)`.
 *
 * Covers come from the engine (`album_cover_art`, `cover_art`): embedded or
 * beside the files for a local library, an HTTP round trip to the server for a
 * remote one. Coil keeps decoded bitmaps in memory and the bytes on disk, keyed
 * by `key`, so the engine is asked once per cover and size.
 */
data class Art(val kind: Kind, val id: Long, val size: ArtSize) {
    enum class Kind { Album, Track, Artist }

    val key get() = "${kind.name.lowercase()}:$id:${size.name.lowercase()}"

    /** How a miss of this cover is kept: artists' never are, as `artist_image` cannot tell a failure from one. */
    val miss get() = Miss(kind, id).takeIf { kind != Kind.Artist }

    companion object {
        fun album(id: Long, size: ArtSize = ArtSize.Thumb) = Art(Kind.Album, id, size)

        fun track(id: Long, size: ArtSize = ArtSize.Thumb) = Art(Kind.Track, id, size)

        /** The artist's photograph, from Wikimedia Commons: one size, the network on a miss. */
        fun artist(id: Long) = Art(Kind.Artist, id, ArtSize.Full)
    }
}

/** What a fetch fails with when the engine has no cover, as against one it could not get. */
class NoArt : Exception("no artwork")

/** A record the engine has no cover for, at any size. */
data class Miss(val kind: Art.Kind, val id: Long)

/**
 * The records the engine answered for and has no cover: asked once, so a grid
 * of them neither asks again nor waits to draw the placeholder. A failed fetch
 * is an error, not an answer, and is not kept. Row ids are reused and a server
 * signed into may hold covers, so the set empties whenever the library or the
 * connection moves.
 */
object ArtMisses {
    private val held = MutableStateFlow<Set<Miss>>(emptySet())
    val misses: StateFlow<Set<Miss>> = held.asStateFlow()

    /** Moves with each clearing, so a fetch begun before one does not record what it may have outlived. */
    private var epoch = 0L

    fun has(miss: Miss?) = miss != null && miss in held.value

    internal fun epoch() = synchronized(this) { epoch }

    internal fun add(miss: Miss, since: Long) = synchronized(this) {
        if (epoch == since) held.update { it + miss }
    }

    internal fun clear() = synchronized(this) {
        epoch++
        held.value = emptySet()
    }
}

class ArtKeyer : Keyer<Art> {
    override fun key(data: Art, options: Options) = data.key
}

class ArtFetcher(
    private val art: Art,
    private val loader: ImageLoader,
    private val mirror: EngineMirror,
) : Fetcher {
    override suspend fun fetch(): FetchResult {
        if (ArtMisses.has(art.miss)) throw NoArt()
        val since = ArtMisses.epoch()
        val disk = loader.diskCache
        disk?.openSnapshot(art.key)?.let { snapshot ->
            return SourceFetchResult(
                ImageSource(snapshot.data, disk.fileSystem, art.key, snapshot),
                null,
                DataSource.DISK,
            )
        }
        val cover = cover() ?: throw NoArt().also { art.miss?.let { ArtMisses.add(it, since) } }
        disk?.openEditor(art.key)?.let { editor ->
            try {
                disk.fileSystem.write(editor.data) { write(cover.data) }
                editor.commitAndOpenSnapshot()?.let { snapshot ->
                    return SourceFetchResult(
                        ImageSource(snapshot.data, disk.fileSystem, art.key, snapshot),
                        cover.mime,
                        DataSource.NETWORK,
                    )
                }
            } catch (e: Exception) {
                editor.abort()
                throw e
            }
        }
        return SourceFetchResult(
            ImageSource(Buffer().write(cover.data), loader.diskCache?.fileSystem ?: okio.FileSystem.SYSTEM),
            cover.mime,
            DataSource.NETWORK,
        )
    }

    private suspend fun cover(): CoverArt? = with(mirror.engine()) {
        when (art.kind) {
            Art.Kind.Album -> albumCoverArt(art.id, art.size.px)
            Art.Kind.Track -> coverArt(art.id, art.size.px)
            Art.Kind.Artist -> artistImage(art.id)
        }
    }

    class Factory(private val mirror: EngineMirror) : Fetcher.Factory<Art> {
        override fun create(data: Art, options: Options, imageLoader: ImageLoader) =
            ArtFetcher(data, imageLoader, mirror)
    }
}

/**
 * Drop cached covers the engine says are stale.
 *
 * SQLite reuses a freed row id, so a cover cached under a deleted album's id
 * would show for the next album given it; a rescanned folder may have a new
 * cover beside its tracks. Each time the library moves, ask the engine what
 * went (`art_evictions`) and forget those covers at every size, and forget
 * every miss, as on each change of connection.
 */
fun followArtEvictions(mirror: EngineMirror, loader: ImageLoader) {
    mirror.launch { mirror.connection.drop(1).collect { ArtMisses.clear() } }
    mirror.launch {
        var seq = artEvictions(0).seq
        mirror.library.drop(1).collect {
            ArtMisses.clear()
            val gone = artEvictions(seq)
            seq = gone.seq
            val arts = gone.albums.map { Art.Kind.Album to it } + gone.tracks.map { Art.Kind.Track to it }
            for ((kind, id) in arts) {
                for (size in ArtSize.entries) {
                    val key = Art(kind, id, size).key
                    loader.diskCache?.remove(key)
                    loader.memoryCache?.keys?.filter { it.key == key }?.forEach { loader.memoryCache?.remove(it) }
                }
            }
        }
    }
}

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
import kotlinx.coroutines.flow.drop
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

    companion object {
        fun album(id: Long, size: ArtSize = ArtSize.Thumb) = Art(Kind.Album, id, size)

        fun track(id: Long, size: ArtSize = ArtSize.Thumb) = Art(Kind.Track, id, size)

        /** The artist's photograph, from Wikimedia Commons: one size, the network on a miss. */
        fun artist(id: Long) = Art(Kind.Artist, id, ArtSize.Full)
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
    override suspend fun fetch(): FetchResult? {
        val disk = loader.diskCache
        disk?.openSnapshot(art.key)?.let { snapshot ->
            return SourceFetchResult(
                ImageSource(snapshot.data, disk.fileSystem, art.key, snapshot),
                null,
                DataSource.DISK,
            )
        }
        val cover = cover() ?: return null
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
 * went (`art_evictions`) and forget those covers at every size.
 */
fun followArtEvictions(mirror: EngineMirror, loader: ImageLoader) {
    mirror.launch {
        var seq = artEvictions(0).seq
        mirror.library.drop(1).collect {
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

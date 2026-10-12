package cc.blit.koan.art

import coil3.ImageLoader
import coil3.decode.DataSource
import coil3.decode.ImageSource
import coil3.disk.DiskCache
import coil3.fetch.FetchResult
import coil3.fetch.Fetcher
import coil3.fetch.SourceFetchResult
import coil3.request.Options
import okio.Buffer
import okio.FileSystem

/** The record has no artwork. A request for it fails, and its error image (the bare ground) is drawn. */
class NoArtworkException(val source: ArtworkSource) : RuntimeException("no artwork for ${source.key}")

/**
 * Coil's way to the engine's cover bytes: a [Fetcher] for [ArtworkSource]. The
 * disk cache is looked at first, under the record's key; a miss asks the
 * [ArtworkStore] (which asks the engine on the IO dispatcher, once per record
 * however many tiles want it) and writes the bytes back. Coil decodes and
 * downsamples to the size the request asked for.
 */
class ArtworkFetcher(
    private val source: ArtworkSource,
    private val options: Options,
    private val store: ArtworkStore,
    private val diskCache: DiskCache?,
) : Fetcher {

    override suspend fun fetch(): FetchResult {
        val key = source.key
        diskCache?.openSnapshot(key)?.let { snapshot ->
            return SourceFetchResult(
                ImageSource(snapshot.data, diskCache.fileSystem, diskCacheKey = key, closeable = snapshot),
                mimeType = null,
                dataSource = DataSource.DISK,
            )
        }
        val art = store.bytes(source) ?: throw NoArtworkException(source)
        diskCache?.let { cache -> write(cache, key, art.data) }
        return SourceFetchResult(
            ImageSource(Buffer().write(art.data), FileSystem.SYSTEM),
            mimeType = art.mime,
            dataSource = DataSource.NETWORK,
        )
    }

    private fun write(cache: DiskCache, key: String, data: ByteArray) {
        val editor = cache.openEditor(key) ?: return
        try {
            cache.fileSystem.write(editor.data) { write(data) }
            editor.commit()
        } catch (e: Exception) {
            runCatching { editor.abort() }
        }
    }

    class Factory(private val store: ArtworkStore) : Fetcher.Factory<ArtworkSource> {
        override fun create(data: ArtworkSource, options: Options, imageLoader: ImageLoader): Fetcher =
            ArtworkFetcher(data, options, store, imageLoader.diskCache)
    }
}

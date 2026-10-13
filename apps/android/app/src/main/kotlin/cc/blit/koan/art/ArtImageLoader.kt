package cc.blit.koan.art

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import cc.blit.koan.engine.KoanProcess
import cc.blit.koan.theme.SleeveColour
import coil3.ImageLoader
import coil3.disk.DiskCache
import coil3.memory.MemoryCache
import coil3.request.ImageRequest
import coil3.request.crossfade
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import okio.Path.Companion.toOkioPath
import java.io.File

/** The memory cache for decoded bitmaps: an eighth of the app's heap budget, between 16 and 96 MB. */
fun artMemoryBytes(memoryClassMb: Int): Long =
    (memoryClassMb.toLong() * 1024 * 1024 / 8).coerceIn(16L * 1024 * 1024, 96L * 1024 * 1024)

/** The disk cache of source bytes, in the cache directory so the system may reclaim it. */
const val ART_DISK_BYTES: Long = 256L * 1024 * 1024

/** Where artwork bytes come from in the app: the engine, once it is up. */
internal object EngineCovers : CoverBytes {
    override suspend fun fetch(source: ArtworkSource, pixels: UInt): ArtworkBytes? {
        // The art may be asked for before the engine has started; wait for it
        // rather than failing, off the main thread (the store calls on IO).
        val engine = KoanProcess.runtime.filterNotNull().first().engine
        val art = when (source) {
            is ArtworkSource.Album -> engine.albumCoverArt(source.id, pixels)
            is ArtworkSource.Track -> engine.coverArt(source.id, pixels)
            is ArtworkSource.Artist -> engine.artistImage(source.id)
        }
        return art?.let { ArtworkBytes(it.data, it.mime) }
    }
}

/** The process's artwork: one store, shared by the image loader and the models that warm it. */
object Artwork {
    val store: ArtworkStore by lazy { ArtworkStore(EngineCovers, colourOf = ::dominantColour) }

    /** A 32-pixel thumbnail of encoded bytes, then the circular hue mean. Null if the bytes are not an image. */
    internal fun dominantColour(data: ByteArray): Int? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(data, 0, data.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        var sample = 1
        while (bounds.outWidth / (sample * 2) >= SleeveColour.SIDE && bounds.outHeight / (sample * 2) >= SleeveColour.SIDE) {
            sample *= 2
        }
        val decoded = BitmapFactory.decodeByteArray(data, 0, data.size, BitmapFactory.Options().apply { inSampleSize = sample })
            ?: return null
        val thumb = Bitmap.createScaledBitmap(decoded, SleeveColour.SIDE, SleeveColour.SIDE, true)
        val pixels = IntArray(SleeveColour.SIDE * SleeveColour.SIDE)
        thumb.getPixels(pixels, 0, SleeveColour.SIDE, 0, 0, SleeveColour.SIDE, SleeveColour.SIDE)
        if (thumb !== decoded) thumb.recycle()
        decoded.recycle()
        return SleeveColour.dominant(pixels)
    }
}

/**
 * The Coil image loader for the app: the [ArtworkFetcher] for [ArtworkSource]s,
 * a bounded memory cache sized from `memoryClass`, and a disk cache.
 */
object ArtImageLoader {
    fun build(context: Context, memoryClassMb: Int, cacheDir: File, store: ArtworkStore = Artwork.store): ImageLoader {
        return ImageLoader.Builder(context)
            .components { add(ArtworkFetcher.Factory(store)) }
            .memoryCache { MemoryCache.Builder().maxSizeBytes(artMemoryBytes(memoryClassMb)).build() }
            .diskCache {
                DiskCache.Builder()
                    .directory(cacheDir.toOkioPath())
                    .maxSizeBytes(ART_DISK_BYTES)
                    .build()
            }
            // The theme's motion is fast and direct: a sleeve arrives, it does not fade.
            .crossfade(false)
            .build()
    }

    /** The disk cache's directory, in the cache directory so the system may reclaim it. */
    fun directory(cacheDir: File): File = cacheDir.resolve("art")

    /**
     * The request for a sleeve drawn about [sizePx] square. Keyed by record and
     * the size bucket it is drawn at, and decoded at that bucket, so a 48 dp row
     * and a 200 dp tile each hold a bitmap their own size and the large one is
     * not kept for both (the way an hour of browsing reached a gigabyte).
     */
    fun request(context: Context, source: ArtworkSource, sizePx: Int): ImageRequest {
        val bucket = ArtworkStore.bucket(sizePx)
        return ImageRequest.Builder(context)
            .data(source)
            .size(bucket)
            .memoryCacheKey("${source.key}@$bucket")
            .build()
    }
}

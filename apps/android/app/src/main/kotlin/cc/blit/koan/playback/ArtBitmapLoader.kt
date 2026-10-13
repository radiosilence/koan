package cc.blit.koan.playback

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.annotation.OptIn
import androidx.core.net.toUri
import androidx.media3.common.util.BitmapLoader
import androidx.media3.common.util.UnstableApi
import cc.blit.koan.art.Art
import cc.blit.koan.art.ArtSize
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.SettableFuture
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * Covers for the media notification, the lock screen and connected
 * controllers, through the app's Coil loader, so they come from the same
 * cache as the covers in the app. `artworkUri` names one as `koan-art://album/<id>`
 * or `koan-art://track/<id>`; see `artUri`.
 */
@OptIn(UnstableApi::class)
class ArtBitmapLoader(private val context: Context, private val scope: CoroutineScope) : BitmapLoader {
    override fun supportsMimeType(mimeType: String) = mimeType.startsWith("image/")

    override fun decodeBitmap(data: ByteArray): ListenableFuture<Bitmap> =
        BitmapFactory.decodeByteArray(data, 0, data.size)?.let { Futures.immediateFuture(it) }
            ?: Futures.immediateFailedFuture(IllegalArgumentException("not an image"))

    override fun loadBitmap(uri: Uri): ListenableFuture<Bitmap> {
        val art = artOf(uri) ?: return Futures.immediateFailedFuture(IllegalArgumentException("not a koan cover: $uri"))
        val done = SettableFuture.create<Bitmap>()
        scope.launch {
            val request = ImageRequest.Builder(context).data(art).allowHardware(false).build()
            val bitmap = (SingletonImageLoader.get(context).execute(request) as? SuccessResult)?.image?.toBitmap()
            if (bitmap != null) done.set(bitmap) else done.setException(IllegalStateException("no cover for $uri"))
        }
        return done
    }

    companion object {
        private const val SCHEME = "koan-art"

        fun artUri(albumId: Long?, trackId: Long?): Uri? = when {
            albumId != null -> "$SCHEME://album/$albumId".toUri()
            trackId != null -> "$SCHEME://track/$trackId".toUri()
            else -> null
        }

        private fun artOf(uri: Uri): Art? {
            if (uri.scheme != SCHEME) return null
            val id = uri.lastPathSegment?.toLongOrNull() ?: return null
            return when (uri.host) {
                "album" -> Art.album(id, ArtSize.Thumb)
                "track" -> Art.track(id, ArtSize.Thumb)
                else -> null
            }
        }
    }
}

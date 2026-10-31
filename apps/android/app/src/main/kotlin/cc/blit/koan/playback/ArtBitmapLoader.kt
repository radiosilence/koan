package cc.blit.koan.playback

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.annotation.OptIn
import androidx.media3.common.util.BitmapLoader
import androidx.media3.common.util.UnstableApi
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import com.google.common.util.concurrent.SettableFuture
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * Covers for the media notification, the lock screen and connected
 * controllers, through the app's Coil loader, so they come from the same
 * cache as the covers in the app. `artworkUri` names one by `ArtProvider`'s
 * URI, which other processes open through the provider; this process reads
 * the cache directly.
 */
@OptIn(UnstableApi::class)
class ArtBitmapLoader(private val context: Context, private val scope: CoroutineScope) : BitmapLoader {
    override fun supportsMimeType(mimeType: String) = mimeType.startsWith("image/")

    override fun decodeBitmap(data: ByteArray): ListenableFuture<Bitmap> =
        BitmapFactory.decodeByteArray(data, 0, data.size)?.let { Futures.immediateFuture(it) }
            ?: Futures.immediateFailedFuture(IllegalArgumentException("not an image"))

    override fun loadBitmap(uri: Uri): ListenableFuture<Bitmap> {
        val art = ArtProvider.artOf(uri) ?: return Futures.immediateFailedFuture(IllegalArgumentException("not a koan cover: $uri"))
        val done = SettableFuture.create<Bitmap>()
        val load = scope.launch {
            // Caught here, so a failed cover settles the future and never
            // reaches the service's scope.
            runCatching {
                val request = ImageRequest.Builder(context).data(art).allowHardware(false).build()
                (SingletonImageLoader.get(context).execute(request) as? SuccessResult)?.image?.toBitmap()
            }.onSuccess { bitmap ->
                if (bitmap != null) done.set(bitmap) else done.setException(IllegalStateException("no cover for $uri"))
            }.onFailure { e ->
                done.setException(e)
                if (e is CancellationException) throw e
            }
        }
        // A load cancelled with the service still settles the future, so
        // Media3 is never left waiting on it; a future the caller cancels
        // stops the load.
        load.invokeOnCompletion { cause -> if (cause is CancellationException) done.setException(cause) }
        done.addListener({ if (done.isCancelled) load.cancel() }, MoreExecutors.directExecutor())
        return done
    }
}

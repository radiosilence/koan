package cc.blit.koan.ui.library

import android.content.Context
import android.graphics.Bitmap
import android.util.LruCache
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.platform.LocalContext
import cc.blit.koan.art.Art
import cc.blit.koan.ui.theme.KoanAccent
import cc.blit.koan.ui.theme.KoanRecordAccent
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Accents worked out from sleeves, by cover: a page visited again takes its
 * colour at once, without decoding and sampling the sleeve again. A cover
 * the engine replaces is forgotten with it (`followArtEvictions`).
 */
object SleeveAccents {
    private val known = LruCache<Art, KoanAccent>(128)

    fun known(art: Art): KoanAccent? = known[art]

    /** `sleeve`'s accent, as the cover `art`; off the main thread, since it samples the pixels. */
    suspend fun of(art: Art, sleeve: Bitmap): KoanAccent =
        known[art] ?: withContext(Dispatchers.Default) { KoanAccent.of(sleeve) }.also { known.put(art, it) }

    /** The accent of the cover `art`, fetching the sleeve if it is not known; `null` for a record with no sleeve. */
    suspend fun of(context: Context, art: Art): KoanAccent? = known[art] ?: sleeve(context, art)?.let { of(art, it) }

    fun forget(art: Art) {
        known.remove(art)
    }
}

/** The cover `art` decoded with readable pixels, or `null` when there is none. */
suspend fun sleeve(context: Context, art: Art): Bitmap? {
    val request = ImageRequest.Builder(context).data(art).allowHardware(false).build()
    return (SingletonImageLoader.get(context).execute(request) as? SuccessResult)?.image?.toBitmap()
}

/** A record's page, or a playlist's after its first record, in that record's accent; the accent in force for `null`. */
@Composable
fun RecordAccent(albumId: Long?, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val art = albumId?.let { Art.album(it) }
    val accent by produceState(art?.let(SleeveAccents::known), art) {
        value = art?.let { SleeveAccents.of(context, it) }
    }
    KoanRecordAccent(accent, content)
}

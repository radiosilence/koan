package cc.blit.koan.ui.library

import android.graphics.Bitmap
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.platform.LocalContext
import cc.blit.koan.art.Art
import cc.blit.koan.ui.theme.KoanRecordAccent
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap

/** A record's page in the record's accent, worked out from its sleeve. */
@Composable
fun RecordAccent(albumId: Long, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val sleeve by produceState<Bitmap?>(null, albumId) {
        val request = ImageRequest.Builder(context).data(Art.album(albumId)).allowHardware(false).build()
        value = (SingletonImageLoader.get(context).execute(request) as? SuccessResult)?.image?.toBitmap()
    }
    KoanRecordAccent(sleeve, content)
}

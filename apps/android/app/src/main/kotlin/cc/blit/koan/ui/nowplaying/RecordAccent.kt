package cc.blit.koan.ui.nowplaying

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import cc.blit.koan.art.Art
import cc.blit.koan.ui.theme.KoanAccent
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The record's accent, read from its sleeve through the cover cache. `null`
 * while the cover loads and for a record without one, which the theme draws
 * in its own mint.
 */
@Composable
fun rememberRecordAccent(art: Art?): KoanAccent? {
    val context = LocalContext.current
    var accent by remember { mutableStateOf<KoanAccent?>(null) }
    LaunchedEffect(art) {
        if (art == null) {
            accent = null
            return@LaunchedEffect
        }
        val request = ImageRequest.Builder(context).data(art).allowHardware(false).build()
        val bitmap = (SingletonImageLoader.get(context).execute(request) as? SuccessResult)?.image?.toBitmap()
        accent = bitmap?.let { withContext(Dispatchers.Default) { KoanAccent.of(it) } }
    }
    return accent
}

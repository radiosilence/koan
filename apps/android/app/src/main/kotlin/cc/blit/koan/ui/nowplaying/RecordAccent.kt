package cc.blit.koan.ui.nowplaying

import android.graphics.Bitmap
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.core.graphics.ColorUtils
import androidx.core.graphics.scale
import cc.blit.koan.art.Art
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The record's colour: the sleeve's most colourful pixels, averaged, then
 * brought to a lightness that reads against the page in the current theme.
 * `null` while the cover loads, for a record without one, or for a grey one.
 */
@Composable
fun rememberRecordAccent(art: Art?): Color? {
    val context = LocalContext.current
    val dark = isSystemInDarkTheme()
    var accent by remember { mutableStateOf<Color?>(null) }
    LaunchedEffect(art, dark) {
        if (art == null) {
            accent = null
            return@LaunchedEffect
        }
        val request = ImageRequest.Builder(context).data(art).allowHardware(false).build()
        val bitmap = (SingletonImageLoader.get(context).execute(request) as? SuccessResult)?.image?.toBitmap()
        accent = bitmap?.let { withContext(Dispatchers.Default) { accentOf(it, dark) } }
    }
    return accent
}

private fun accentOf(bitmap: Bitmap, dark: Boolean): Color? {
    val small = bitmap.scale(24, 24)
    val hsl = FloatArray(3)
    var r = 0.0
    var g = 0.0
    var b = 0.0
    var weight = 0.0
    for (y in 0 until small.height) {
        for (x in 0 until small.width) {
            val pixel = small.getPixel(x, y)
            ColorUtils.colorToHSL(pixel, hsl)
            // Colour counts for more than area: a small red label on a grey
            // sleeve is what the record is remembered by.
            val w = (hsl[1] * (1 - Math.abs(hsl[2] * 2 - 1))).toDouble().let { it * it }
            r += android.graphics.Color.red(pixel) * w
            g += android.graphics.Color.green(pixel) * w
            b += android.graphics.Color.blue(pixel) * w
            weight += w
        }
    }
    if (weight < 1.0) return null
    ColorUtils.RGBToHSL((r / weight).toInt(), (g / weight).toInt(), (b / weight).toInt(), hsl)
    hsl[1] = hsl[1].coerceIn(0.35f, 0.85f)
    hsl[2] = if (dark) hsl[2].coerceIn(0.6f, 0.75f) else hsl[2].coerceIn(0.3f, 0.42f)
    return Color(ColorUtils.HSLToColor(hsl))
}

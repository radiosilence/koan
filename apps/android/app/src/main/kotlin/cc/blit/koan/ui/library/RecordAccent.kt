package cc.blit.koan.ui.library

import android.graphics.Bitmap
import androidx.compose.animation.animateColorAsState
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import androidx.core.graphics.ColorUtils
import androidx.core.graphics.get
import androidx.core.graphics.scale
import cc.blit.koan.art.Art
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap

/**
 * A record's page in the record's colour: the sleeve's dominant hue, held
 * to a lightness that reads against the page, tints the controls on it.
 * Greys and near-blacks leave the theme's own accent.
 */
@Composable
fun RecordAccent(albumId: Long, content: @Composable () -> Unit) {
    val context = LocalContext.current
    var sleeve by remember(albumId) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(albumId) {
        val request = ImageRequest.Builder(context).data(Art.album(albumId)).allowHardware(false).build()
        sleeve = (SingletonImageLoader.get(context).execute(request) as? SuccessResult)?.image?.toBitmap()
    }
    val scheme = MaterialTheme.colorScheme
    val dark = scheme.surface.luminance() < 0.5f
    val target = remember(sleeve, dark) { sleeve?.let { accent(it, dark) } } ?: scheme.primary
    val primary by animateColorAsState(target, label = "record accent")
    val container by animateColorAsState(primary.copy(alpha = 0.18f), label = "record container")
    MaterialTheme(
        colorScheme = scheme.copy(primary = primary, secondaryContainer = container),
        typography = MaterialTheme.typography,
        shapes = MaterialTheme.shapes,
        content = content,
    )
}

private fun accent(sleeve: Bitmap, dark: Boolean): Color? {
    val small = sleeve.scale(24, 24)
    var x = 0.0
    var y = 0.0
    var weight = 0.0
    val hsl = FloatArray(3)
    for (px in 0 until 24) for (py in 0 until 24) {
        ColorUtils.colorToHSL(small[px, py], hsl)
        val w = (hsl[1] * (1 - kotlin.math.abs(2 * hsl[2] - 1))).toDouble()
        val angle = Math.toRadians(hsl[0].toDouble())
        x += kotlin.math.cos(angle) * w
        y += kotlin.math.sin(angle) * w
        weight += w
    }
    if (weight / (24 * 24) < 0.08) return null
    val hue = ((Math.toDegrees(kotlin.math.atan2(y, x)) + 360) % 360).toFloat()
    return Color(ColorUtils.HSLToColor(floatArrayOf(hue, 0.55f, if (dark) 0.72f else 0.38f)))
}
